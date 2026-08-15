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

    let base = config_base(environment)?;

    // "default" is spelled by absence rather than by name, so it does not get a directory.
    match lookup("HERDR_SESSION") {
        Some(session) if session != "default" => {
            Some(format!("{base}/sessions/{session}/herdr.sock"))
        }
        _ => Some(format!("{base}/herdr.sock")),
    }
}

/// The name of the herdr session Muster runs its own daemon under.
///
/// A named session rather than a path of Muster's invention, because herdr already has this
/// concept and spells it the same way everywhere: `herdr --session muster pane list` reaches
/// this daemon from a terminal, which is the whole escape hatch, and it needs no explaining
/// to anyone who already knows herdr.
pub const OWN_SESSION: &str = "muster";

/// Where Muster's own daemon listens.
///
/// Not [`discover_socket_path`], and the difference is the point. That answers "where is the
/// daemon this machine's herdr would use", which is whatever the user last started - a daemon
/// of unknown version, whose behaviour Muster's corpus says nothing about. This answers
/// "where is ours", which is a daemon Muster started from the binary it shipped.
///
/// `HERDR_SOCKET_PATH` is deliberately not read: it points at somebody else's daemon, and a
/// window that silently took it would be back to attaching whatever answered. Naming a
/// `socket` in Muster's own config file is how you ask for that on purpose.
pub fn own_socket_path(environment: &BTreeMap<String, String>) -> Option<String> {
    Some(format!("{}/sessions/{OWN_SESSION}/herdr.sock", config_base(environment)?))
}

/// The config file Muster's daemon will read, which is the user's own herdr config.
///
/// Reported rather than changed, and that is a limitation rather than a decision. Muster's
/// daemon is a session of its own but not a world of its own: it reads whatever
/// `config.toml` the user wrote for their own herdr, so a `default_shell` or a `shell_mode`
/// changed for their terminal silently changes what Muster's panes do - and the corpus was
/// recorded against neither.
///
/// It cannot be isolated from here. herdr resolves its config directory from
/// `XDG_CONFIG_HOME` and nothing else (`config/io.rs`, `config_dir`), and a pane's process
/// inherits the daemon's environment - so pointing the daemon at a private directory would
/// point every pane's git, editor and shell at one too. That is a worse fault than the one it
/// fixes, so what is left is saying which file is in play, where somebody debugging a
/// surprising pane will see it.
pub fn config_file(environment: &BTreeMap<String, String>) -> Option<String> {
    Some(format!("{}/config.toml", config_base(environment)?))
}

/// herdr's configuration directory, which is also where its sockets live.
///
/// None when the environment says nothing about where home is. Returning a plausible-looking
/// path would turn "there is no daemon here" into a connection error somewhere further from
/// the cause.
fn config_base(environment: &BTreeMap<String, String>) -> Option<String> {
    let lookup = |name: &str| environment.get(name).filter(|value| !value.is_empty());
    match (lookup("XDG_CONFIG_HOME"), lookup("HOME")) {
        (Some(xdg), _) => Some(format!("{xdg}/herdr")),
        (None, Some(home)) => Some(format!("{home}/.config/herdr")),
        (None, None) => None,
    }
}
