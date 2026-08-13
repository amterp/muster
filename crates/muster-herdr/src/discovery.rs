//! Where a herdr daemon is listening, by herdr's own rules.

use std::collections::BTreeMap;

/// Ported from `src/session.rs` and `src/config/io.rs`, in precedence order:
/// `HERDR_SOCKET_PATH` wins outright; otherwise a named `HERDR_SESSION` selects a
/// per-session socket; otherwise the default session's. The base directory is
/// `$XDG_CONFIG_HOME/herdr` or `~/.config/herdr`.
///
/// A release herdr uses `herdr`; a debug build uses `herdr-dev`. Muster looks for the
/// release directory, since that is what a person runs.
///
/// Reimplemented rather than asked for, which makes it exactly the kind of thing that
/// drifts silently: getting it wrong does not crash Muster, it falls back to guessed
/// encodings and arrow keys stop working in pagers while everything else looks fine.
///
/// Takes the environment rather than reading it, so the rules are testable without one.
pub fn discover_socket_path(environment: &BTreeMap<String, String>) -> Option<String> {
    let lookup = |name: &str| environment.get(name).filter(|value| !value.is_empty());

    if let Some(explicit) = lookup("HERDR_SOCKET_PATH") {
        return Some(explicit.clone());
    }

    let base = match (lookup("XDG_CONFIG_HOME"), lookup("HOME")) {
        (Some(xdg), _) => format!("{xdg}/herdr"),
        (None, Some(home)) => format!("{home}/.config/herdr"),
        // Returning a plausible-looking path would turn "there is no daemon here" into a
        // connection error somewhere further from the cause.
        (None, None) => return None,
    };

    // "default" is spelled by absence rather than by name, so it does not get a directory.
    match lookup("HERDR_SESSION") {
        Some(session) if session != "default" => {
            Some(format!("{base}/sessions/{session}/herdr.sock"))
        }
        _ => Some(format!("{base}/herdr.sock")),
    }
}
