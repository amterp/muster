//! What a new pane runs, and with what environment. Pure: `pty.rs` starts it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

/// Variables the daemon sets itself on every pane, which an inherited or requested copy never
/// overrides: a pane must not be told it is some other pane.
const PANE_NAME: &str = "MUSTER_PANE";

/// Where a pane's command travels to its shell (`argv` says why), which the command and
/// everything it starts can also read.
pub(crate) const PANE_COMMAND: &str = "MUSTER_PANE_COMMAND";

/// Variables dropped from what the daemon inherited. A daemon a developer started by hand from
/// inside a Muster pane carries that pane's name, window and command, and a pane that inherited
/// them would drive the wrong window. The requested environment supplies the right
/// `MUSTER_SOCKET`; the daemon supplies the others.
const NOT_INHERITED: [&str; 3] = [PANE_NAME, "MUSTER_SOCKET", PANE_COMMAND];

/// The argv a pane starts with.
///
/// An interactive shell, a login one unless asked otherwise (MIP-3, section 3). A command runs
/// through that shell, which then replaces itself with an interactive shell, so the command
/// starts with no typed input for a program to discard, and the pane drops to a shell when the
/// command exits.
///
/// The command reaches the shell in [`PANE_COMMAND`] and runs through `eval`, rather than being
/// written into the script. Written in, an open quote, a trailing backslash or an unfinished
/// heredoc would swallow the `exec` after it and the pane would close. Through `eval` it is a
/// shell error like any other, and the `exec` still runs. `eval "$VARIABLE"` means the same in
/// sh, bash, zsh and fish.
pub(crate) fn argv(shell: &str, login: bool, runs_command: bool) -> Vec<String> {
    let mut argv = vec![shell.to_string()];
    let flags: &[&str] = if login { &["-l", "-i"] } else { &["-i"] };
    argv.extend(flags.iter().map(|flag| (*flag).to_string()));
    if runs_command {
        argv.push("-c".to_string());
        argv.push(format!("eval \"${PANE_COMMAND}\"\nexec {} {}", quote(shell), flags.join(" ")));
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
    command: Option<&str>,
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
        if name != PANE_NAME && name != PANE_COMMAND {
            set(name, value);
        }
    }
    set("COLORTERM", "truecolor");
    set(PANE_NAME, pane);
    if let Some(command) = command {
        set(PANE_COMMAND, command);
    }
    environment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_alone_is_interactive_and_login_unless_asked_otherwise() {
        assert_eq!(argv("/bin/zsh", true, false), ["/bin/zsh", "-l", "-i"]);
        assert_eq!(argv("/bin/zsh", false, false), ["/bin/zsh", "-i"]);
    }

    #[test]
    fn a_command_is_evaluated_and_then_the_shell_becomes_itself() {
        assert_eq!(
            argv("/bin/zsh", true, true),
            ["/bin/zsh", "-l", "-i", "-c", "eval \"$MUSTER_PANE_COMMAND\"\nexec '/bin/zsh' -l -i"]
        );
        assert_eq!(
            argv("/opt/it's/fish", false, true),
            [
                "/opt/it's/fish",
                "-i",
                "-c",
                "eval \"$MUSTER_PANE_COMMAND\"\nexec '/opt/it'\\''s/fish' -i"
            ]
        );
    }

    #[test]
    fn the_pane_is_named_by_the_daemon_and_nothing_else() {
        let inherited = vec![
            ("PATH".into(), "/bin".into()),
            ("MUSTER_PANE".into(), "p-stale".into()),
            ("MUSTER_SOCKET".into(), "/stale.sock".into()),
            ("MUSTER_PANE_COMMAND".into(), "stale".into()),
            ("COLORTERM".into(), "24bit".into()),
        ];
        let requested = HashMap::from([
            ("MUSTER_SOCKET".to_string(), "/window.sock".to_string()),
            ("MUSTER_PANE".to_string(), "p-requested".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        let mut environment = environment(&inherited, &requested, "p1", None);
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
