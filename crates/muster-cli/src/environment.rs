//! What a pane is told about the window it is drawn in.
//!
//! Two variables, set by Muster in the `env` of the very request that creates a pane, and the
//! whole of how a program inside one can drive its own window: which pane it is, and which
//! Muster to tell.
//!
//! Spelled out here rather than imported. The app sets them from `muster-daemon-client`, and the
//! CLI is a pure client that links neither the daemon client nor anything behind it. So the two
//! spellings are separate on purpose, and `tests/cli/pane_variables.rs` fails if they ever stop
//! matching.

/// What a pane reads to find out which pane it is.
pub const PANE_NAME: &str = "MUSTER_PANE";

/// What a pane reads to find the window it is in.
///
/// Set per window rather than looked up, because a machine can have several Musters open and a
/// pane belongs to exactly one. A pane on a machine attached over ssh is told the path there
/// that the window's ssh master forwards back to it. Named after the window's process, so a pane
/// whose window has quit asks the windows beside it instead (`dial::siblings`).
pub const WINDOW_SOCKET: &str = "MUSTER_SOCKET";

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
