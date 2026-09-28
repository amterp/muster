//! Asking an older daemon to hand its panes to this build's (MIP-3, section 10).
//!
//! A newer app used to adopt whatever daemon answered, so a daemon fix reached a machine only
//! when that daemon restarted, which ends every agent in it. The app asks instead, once, when it
//! adopts a daemon older than the one it carries; a daemon that refuses keeps serving as it was.

use std::path::Path;
use std::time::Duration;

use muster_daemon_proto::launch::LAUNCH_PATIENCE;
use muster_daemon_proto::{self as proto, ConnectionKind, Welcome};

use crate::control::{Control, Unanswered};

/// The version of the daemon this build carries: every crate in the workspace shares one.
pub const OURS: &str = env!("CARGO_PKG_VERSION");

/// How a running daemon's version compares with the one this build carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Older,
    Same,
    Newer,
    /// A version that does not read as one.
    Unreadable,
}

/// How a daemon saying it is `running` compares with this build's.
pub fn age(running: &str) -> Age {
    age_against(running, OURS)
}

/// The same against `ours`, which is what a test names.
///
/// `major.minor.patch` compared as numbers. Anything after a `-` or a `+` is ignored, so two
/// development builds of one version are the same: handing over between them at every launch
/// would restart every pane's reader for no fix. A version that is not three numbers is
/// `Unreadable`, and nothing is asked of a daemon whose version cannot be read.
pub fn age_against(running: &str, ours: &str) -> Age {
    match (numbers(running), numbers(ours)) {
        (Some(running), Some(ours)) => match running.cmp(&ours) {
            std::cmp::Ordering::Less => Age::Older,
            std::cmp::Ordering::Equal => Age::Same,
            std::cmp::Ordering::Greater => Age::Newer,
        },
        _ => Age::Unreadable,
    }
}

fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let read = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(read)
}

/// How long a handoff may take to answer: the successor's launch, which is waited for first,
/// and then the exchange, whose steps each have ten seconds.
const ANSWER_PATIENCE: Duration = LAUNCH_PATIENCE.saturating_add(Duration::from_mins(1));

/// Asks the daemon on `socket` to hand every pane to `program`, and waits for it to answer.
///
/// Returns who serves the socket afterwards. The follower already connected there goes on by
/// itself: it hears `Replaced` and connects again, as it does after any handoff. A refusal comes
/// back with the daemon's reason, and the daemon that refused goes on exactly as it was.
pub fn hand_over(socket: &Path, program: &Path, data: Option<&Path>) -> Result<Welcome, String> {
    let control = Control::open(socket, "muster handover", |_, _| {})
        .map_err(|error| format!("could not reach the daemon to ask ({error})"))?;
    let before = control.welcome().instance;
    let answered = control.replace(program, data).wait(ANSWER_PATIENCE);
    drop(control);
    match answered {
        Ok(answer) if answer.outcome() == proto::Outcome::Done => serving(socket, before),
        Ok(answer) => Err(if answer.reason.is_empty() {
            format!("it answered {:?} and gave no reason", answer.outcome())
        } else {
            answer.reason
        }),
        // The old daemon exits once it has answered, and a connection it ended first may have
        // taken the answer with it; who serves now says whether it went through.
        Err(Unanswered::Ended) => serving(socket, before),
        Err(Unanswered::TimedOut) => Err(format!(
            "it did not answer within {}s, and is still in charge of its panes",
            ANSWER_PATIENCE.as_secs()
        )),
    }
}

/// Who serves `socket` after a handoff, which went through when that is no longer `before`.
fn serving(socket: &Path, before: u64) -> Result<Welcome, String> {
    let (_, welcome) = crate::dial(socket, ConnectionKind::Control, "muster handover")
        .map_err(|error| format!("nothing answered on the socket after the handoff ({error})"))?;
    if welcome.instance == before {
        return Err("the same daemon is still serving, so the handoff did not happen".to_string());
    }
    Ok(welcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers_and_not_as_text() {
        assert_eq!(age_against("0.9.0", "0.10.0"), Age::Older);
        assert_eq!(age_against("0.10.0", "0.9.0"), Age::Newer);
        assert_eq!(age_against("0.10.0", "0.10.0"), Age::Same);
        assert_eq!(age_against("1.0.0", "0.99.9"), Age::Newer);
        assert_eq!(age_against("0.10.1", "0.10.2"), Age::Older);
    }

    #[test]
    fn a_prerelease_of_the_same_numbers_is_the_same() {
        // Two development builds share a version; handing over between them at every launch
        // would churn every pane and deliver no fix.
        assert_eq!(age_against("0.10.0-dev", "0.10.0"), Age::Same);
        assert_eq!(age_against("0.10.0+abc", "0.10.0"), Age::Same);
    }

    #[test]
    fn a_version_that_does_not_read_is_left_alone() {
        assert_eq!(age_against("", "0.10.0"), Age::Unreadable);
        assert_eq!(age_against("ten", "0.10.0"), Age::Unreadable);
        assert_eq!(age_against("0.10", "0.10.0"), Age::Unreadable);
    }
}
