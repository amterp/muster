//! Waiting for something to become true, rather than sleeping and hoping.
//!
//! Every daemon-backed test needs this, and before it lived here each of them wrote its own:
//! twenty-four copies across four crates, with deadlines of two, ten, fifteen, twenty and
//! thirty seconds and poll intervals from one millisecond to fifty, and not one of the outliers
//! carrying a reason. `docs/testing.md` names that as the hazard - "a wait sized by guesswork
//! is the flake this rule exists to prevent" - and a copy per file is how the guesswork
//! spreads, because the way a test gets written is by copying the nearest one.
//!
//! Here rather than in a crate of its own because everything that needs it already depends on
//! this one, which is what makes there being exactly one of these cheap.

use std::path::Path;
use std::time::{Duration, Instant};

/// How long a wait may take before it counts as never happening.
///
/// One number for the whole suite, and deliberately a generous one. A deadline here bounds a
/// failure; it is not a dial to tune, and the evidence is the subscription bug of
/// 2026-08-17: under load that wedge produced runs finishing in 0.25 to 1.0 seconds or runs
/// sitting at exactly the deadline, with nothing in between. No number would have fixed it and
/// the shape of the distribution is what told the truth. So this is longer than every
/// per-file deadline it replaced, which means no wait it took over can have been made tighter.
///
/// A wait that genuinely needs longer uses [`until_within`] and says why there.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// How often the condition is asked. Not a wait: what the test waits for is the condition, and
/// the deadline decides only how long it takes to fail (`docs/testing.md`).
const INTERVAL: Duration = Duration::from_millis(10);

/// What was true instead, said at the moment the wait gives up.
///
/// A trait rather than an `Option<F>` so that both spellings read as an argument: `()` where
/// there is nothing to add beyond the name of the wait, and a closure where there is. The
/// closure runs only if the wait fails, so it is free to take a lock and format what it finds.
///
/// The point of it being in the signature at all is that a timeout which says a condition never
/// came true, and nothing about what was true instead, sends whoever hit it back to add exactly
/// this and run again.
pub trait Detail {
    fn detail(self) -> String;
}

impl Detail for () {
    fn detail(self) -> String {
        String::new()
    }
}

impl<F: FnOnce() -> String> Detail for F {
    fn detail(self) -> String {
        self()
    }
}

/// Waits for a condition, or fails naming it and whatever `detail` has to add.
pub fn until(what: &str, ready: impl FnMut() -> bool, detail: impl Detail) {
    until_within(what, PATIENCE, ready, detail);
}

/// The same, for a wait that genuinely needs longer or shorter than [`PATIENCE`].
///
/// Say why at the call site. An allowance with no reason beside it is the guesswork this module
/// exists to collect in one place.
pub fn until_within(
    what: &str,
    allowance: Duration,
    mut ready: impl FnMut() -> bool,
    detail: impl Detail,
) {
    let deadline = Instant::now() + allowance;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(INTERVAL);
    }
    let detail = detail.detail();
    let said = if detail.is_empty() { String::new() } else { format!("\n  {detail}") };
    panic!("timed out after {allowance:?} waiting for {what}.{said}");
}

/// Waits for an answer, and hands it back.
///
/// For a wait whose condition *is* the thing being waited for - the latest view, the pane a
/// daemon just made - so that a caller does not have to ask twice and explain to the reader why
/// the second ask cannot fail.
pub fn until_some<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if let Some(answer) = ready() {
            return answer;
        }
        std::thread::sleep(INTERVAL);
    }
    panic!("timed out after {PATIENCE:?} waiting for {what}, which never arrived");
}

/// Waits for a file something in a pane was asked to write.
///
/// Non-empty rather than merely present, because a shell redirecting into a path creates it
/// before the command it is running has said anything.
pub fn until_file(path: &Path, what: &str) {
    until(
        what,
        || std::fs::read_to_string(path).is_ok_and(|text| !text.is_empty()),
        || {
            format!(
                "{} was never written.\n  Impact: the pane exists and is sitting at its own \
                 prompt, which looks exactly like a command that ran and printed nothing.",
                path.display()
            )
        },
    );
}
