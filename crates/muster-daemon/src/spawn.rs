//! What a new pane runs, and with what environment. Pure: `pty.rs` starts it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

/// Variables the daemon sets itself on every pane, which an inherited or requested copy never
/// overrides: a pane must not be told it is some other pane.
const PANE_NAME: &str = "MUSTER_PANE";

/// Variables dropped from what the daemon inherited. A daemon a developer started by hand from
/// inside a Muster pane carries that pane's name and window, and a pane that inherited them
/// would drive the wrong window. The requested environment supplies the right `MUSTER_SOCKET`;
/// the daemon supplies `MUSTER_PANE`.
const NOT_INHERITED: [&str; 2] = [PANE_NAME, "MUSTER_SOCKET"];

/// The argv a pane starts with.
///
/// An interactive shell, a login one unless asked otherwise (MIP-3, section 3). A command runs
/// through that shell, which then replaces itself with an interactive shell, so the command
/// starts with no typed input for a program to discard, and the pane drops to a shell when the
/// command exits. A newline rather than `;` separates the two, so a command ending in `&` or in
/// a comment still leaves the `exec` intact.
pub(crate) fn argv(shell: &str, login: bool, command: Option<&str>) -> Vec<String> {
    let mut argv = vec![shell.to_string()];
    let flags: &[&str] = if login { &["-l", "-i"] } else { &["-i"] };
    argv.extend(flags.iter().map(|flag| (*flag).to_string()));
    if let Some(command) = command {
        argv.push("-c".to_string());
        argv.push(format!("{command}\nexec {} {}", quote(shell), flags.join(" ")));
    }
    argv
}

/// `text` as one word to a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A pane's environment: what the daemon inherited, less what names another pane, plus what the
/// request asked for, plus what the daemon sets itself.
///
/// `TERM` is not decided here. Until the daemon carries its terminfo entry (a card of its own), a
/// pane has whatever `TERM` the daemon inherited.
pub(crate) fn environment(
    inherited: &[(OsString, OsString)],
    requested: &HashMap<String, String>,
    pane: &str,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = inherited
        .iter()
        .filter(|(name, _)| !NOT_INHERITED.iter().any(|dropped| name == OsStr::new(dropped)))
        .cloned()
        .collect();
    let mut set = |name: &str, value: &str| {
        environment.retain(|(existing, _)| existing != OsStr::new(name));
        environment.push((name.into(), value.into()));
    };
    let mut requested: Vec<_> = requested.iter().collect();
    requested.sort();
    for (name, value) in requested {
        if name != PANE_NAME {
            set(name, value);
        }
    }
    set("COLORTERM", "truecolor");
    set(PANE_NAME, pane);
    environment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_alone_is_interactive_and_login_unless_asked_otherwise() {
        assert_eq!(argv("/bin/zsh", true, None), ["/bin/zsh", "-l", "-i"]);
        assert_eq!(argv("/bin/zsh", false, None), ["/bin/zsh", "-i"]);
    }

    #[test]
    fn a_command_runs_and_then_becomes_the_same_shell() {
        assert_eq!(
            argv("/bin/zsh", true, Some("claude --resume")),
            ["/bin/zsh", "-l", "-i", "-c", "claude --resume\nexec '/bin/zsh' -l -i"]
        );
        assert_eq!(
            argv("/opt/it's/fish", false, Some("sleep 1 &")),
            ["/opt/it's/fish", "-i", "-c", "sleep 1 &\nexec '/opt/it'\\''s/fish' -i"]
        );
    }

    #[test]
    fn the_pane_is_named_by_the_daemon_and_nothing_else() {
        let inherited = vec![
            ("PATH".into(), "/bin".into()),
            ("MUSTER_PANE".into(), "p-stale".into()),
            ("MUSTER_SOCKET".into(), "/stale.sock".into()),
            ("COLORTERM".into(), "24bit".into()),
        ];
        let requested = HashMap::from([
            ("MUSTER_SOCKET".to_string(), "/window.sock".to_string()),
            ("MUSTER_PANE".to_string(), "p-requested".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        let mut environment = environment(&inherited, &requested, "p1");
        environment.sort();
        let expected: Vec<(OsString, OsString)> = [
            ("COLORTERM", "truecolor"),
            ("MUSTER_PANE", "p1"),
            ("MUSTER_SOCKET", "/window.sock"),
            ("PATH", "/usr/bin"),
        ]
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect();
        assert_eq!(environment, expected);
    }
}
