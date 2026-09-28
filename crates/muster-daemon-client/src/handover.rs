//! Asking an older daemon to hand its panes to this build's (MIP-3, section 10).

use std::path::Path;

use muster_daemon_proto::Welcome;

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

pub fn age_against(_running: &str, _ours: &str) -> Age {
    Age::Same
}

/// Asks the daemon on `socket` to hand every pane to `program`, and waits for it to answer.
pub fn hand_over(_socket: &Path, _program: &Path, _data: Option<&Path>) -> Result<Welcome, String> {
    Err("not yet".to_string())
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
