//! The one thread that notices a pane nobody ever dialed.
//!
//! `muster_core::typeable` decides which panes have waited too long; this holds the clock and
//! the thread that asks it, and turns the answer into problems the roster draws. Split that
//! way because the deadline is the only part of this a case cannot reach: a fold over (pane,
//! when it started, what time it is now) is testable, and a thread parked on a condvar is not.
//!
//! **Only this module raises or clears a pane's problem.** The call sites in `session.rs` do
//! nothing but record into `WAITING` and knock, and that is a lock rule rather than a style
//! preference. `raise_problem` takes `PROBLEMS` and then `SESSION`, while `open_channel` runs
//! holding `SESSION` and wants `WAITING` - so a call site that raised while holding `WAITING`
//! would be the other half of an AB/BA deadlock against the thread that accepted a
//! connection. One writer removes the ordering question instead of documenting an answer to
//! it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use muster_core::PaneKey;
use muster_core::diagnostics::{clock, log, poison};
use muster_core::fields;
use muster_core::problems::Severity;
use muster_core::typeable::Waiting;

use crate::session;

/// How long a pane may render before its silence is worth reporting, in milliseconds.
///
/// `tools/smoke-launch.py` waits 2.0s after `app.ready` for a healthy bridge to "start, dial
/// back and paint", so five seconds is about two and a half times the budget a working launch
/// is already known to fit inside - long enough that a machine under load does not get
/// accused, short enough that nobody has typed into a deaf pane and drawn their own
/// conclusions first.
const DEADLINE_MS: u64 = 5_000;

/// The deadline this run is using, in nanoseconds, read from the environment once.
///
/// `MUSTER_TYPEABLE_DEADLINE_MS` overrides it, and `0` switches the watch off. It is here
/// rather than in `config.toml` because its reason to exist is the suite: the seam's own tests
/// attach a real daemon and never start a bridge, so every pane in them is genuinely
/// untypeable, and proving that end to end should not cost five seconds of gate. A run that
/// has no bridges to wait for can say so, which is the same knob from the other side.
static DEADLINE: LazyLock<u64> = LazyLock::new(|| {
    let Ok(spelled) = std::env::var("MUSTER_TYPEABLE_DEADLINE_MS") else {
        return DEADLINE_MS * 1_000_000;
    };
    match spelled.trim().parse::<u64>() {
        Ok(millis) => {
            log::info("typeable.deadline.overridden", fields! { "millis" => millis.to_string() });
            millis.saturating_mul(1_000_000)
        }
        Err(error) => {
            log::warn(
                "typeable.deadline.unreadable",
                fields! {
                    "value" => spelled,
                    "detail" => error.to_string(),
                    "impact" => format!(
                        "MUSTER_TYPEABLE_DEADLINE_MS is not a whole number of milliseconds, so \
                         the default of {DEADLINE_MS}ms is in force and a pane that never \
                         becomes typeable is reported after that instead"
                    ),
                    "check" => "write a count of milliseconds, or 0 to stop watching for it",
                },
            );
            DEADLINE_MS * 1_000_000
        }
    }
});

/// The panes being waited on, and the door the thread is knocked on.
///
/// A leaf lock: nothing is called while it is held. See the module comment for why that
/// matters rather than merely being tidy.
static WAITING: Mutex<Waiting> = Mutex::new(Waiting::new());
static KNOCK: Condvar = Condvar::new();
static WATCHING: AtomicBool = AtomicBool::new(false);

/// A pane's socket is bound, so its bridge is expected from now.
///
/// Also how a wait restarts, for a pane whose surface was thrown away and built again.
pub(crate) fn opened(pane: PaneKey) {
    poison::lock(&WAITING, "typeable").opened(pane, clock::monotonic_now());
    start();
}

/// A bridge dialed in.
pub(crate) fn typeable(pane: &PaneKey) {
    poison::lock(&WAITING, "typeable").typeable(pane);
    KNOCK.notify_all();
}

/// The pane is gone, so nothing is owed about it.
pub(crate) fn closed(pane: &PaneKey) {
    poison::lock(&WAITING, "typeable").closed(pane);
    KNOCK.notify_all();
}

/// Forgets every pane being waited on, for a process starting over.
///
/// The thread stays. It is parked on the condvar with nothing owed, which is what it does
/// between panes anyway, and stopping it would mean a way to start a second one - which is a
/// mechanism nothing in a shipped window would ever use.
pub(crate) fn forget_everything() {
    *poison::lock(&WAITING, "typeable") = Waiting::new();
    KNOCK.notify_all();
}

/// Starts the thread that watches the clock, once per process.
///
/// Lazily, because a run that never opens a pane should not carry a thread, and parked on a
/// condvar rather than ticking, because a window whose panes are all typeable has nothing for
/// it to do and an idle window should cost no wakeups at all.
fn start() {
    if *DEADLINE == 0 {
        return;
    }
    if WATCHING.swap(true, Ordering::AcqRel) {
        KNOCK.notify_all();
        return;
    }
    std::thread::spawn(watch);
}

fn watch() {
    let deadline = *DEADLINE;
    loop {
        let reported = {
            let mut waiting = poison::lock(&WAITING, "typeable");
            waiting.reconcile(clock::monotonic_now(), deadline)
        };
        for (key, detail) in reported.raise {
            session::raise_problem(&key, Severity::Error, &detail);
        }
        for key in reported.clear {
            session::clear_problem(&key);
        }

        // Asked again under the guard this waits on, rather than reused from above. A pane
        // opened while those problems were being published would otherwise be slept through -
        // its knock lands while nobody is waiting, and on a quiet window nothing else arrives
        // to wake this up. Holding the lock across the wait is what makes a later knock
        // reliable.
        let waiting = poison::lock(&WAITING, "typeable");
        match waiting.next_wake(clock::monotonic_now(), deadline) {
            Some(nanos) => drop(KNOCK.wait_timeout(waiting, Duration::from_nanos(nanos))),
            // Nothing more happens on the clock alone: every pane still waiting is already
            // reported, and the two things that can change that both knock.
            None => drop(KNOCK.wait(waiting)),
        }
    }
}
