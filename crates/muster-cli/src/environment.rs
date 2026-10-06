//! What a pane is told about the window it is drawn in.
//!
//! Two variables, set by Muster in the `env` of the very request that creates a pane, and the
//! whole of how a program inside one can drive its own window: which pane it is, and which
//! Muster to tell. Spelled once, in `muster-daemon-launch`, which the app sets them from and
//! this reads them through: a drift would leave every running pane under the old name, so an
//! agent asking which pane it is would silently act on whichever one has the keyboard.

pub use muster_daemon_launch::environment::{PANE_NAME, WINDOW_SOCKET};

/// Where Muster keeps everything that is its own rather than the user's.
///
/// The same rule `MusterHome.swift` applies, reimplemented rather than asked for, because the
/// CLI is a separate program and there is nobody to ask before it has found a window. Kept to
/// one function so the duplication is one place a reader can compare.
pub fn muster_home(environment: &std::collections::BTreeMap<String, String>) -> Option<String> {
    if let Some(explicit) = environment.get("MUSTER_HOME").filter(|home| !home.is_empty()) {
        return Some(explicit.clone());
    }
    let home = environment.get("HOME").filter(|home| !home.is_empty())?;
    Some(format!("{}/.muster", home.trim_end_matches('/')))
}
