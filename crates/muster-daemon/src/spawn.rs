//! What a new pane runs, and with what environment. Pure: `pty.rs` starts it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

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

/// Variables from a Ghostty the daemon was started in - its resources, its binary, its surface -
/// which describe that terminal rather than this pane. A requested copy is still honored.
const GHOSTTY_PREFIX: &str = "GHOSTTY_";

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
/// sh, bash, zsh and fish - but see [`evaluate`] for how each is asked.
pub(crate) fn argv(shell: &str, login: bool, runs_command: bool) -> Vec<String> {
    let mut argv = vec![shell.to_string()];
    let flags: &[&str] = if login { &["-l", "-i"] } else { &["-i"] };
    argv.extend(flags.iter().map(|flag| (*flag).to_string()));
    if runs_command {
        argv.push("-c".to_string());
        argv.push(format!(
            "{} \"${PANE_COMMAND}\"\nexec {} {}",
            evaluate(shell),
            quote(shell),
            flags.join(" ")
        ));
    }
    argv
}

/// How `shell` is told to evaluate the command.
///
/// `eval` is a special builtin in a POSIX shell, and a syntax error inside one abandons the rest
/// of the script: dash, Debian's and Ubuntu's `/bin/sh`, then never reaches the `exec`, and the
/// pane is left in the shell that ran the command rather than a login shell of its own. `command`
/// takes the special status away, so the error is an ordinary failure and the script goes on.
/// zsh and fish read `command eval` as an external program called `eval`, and neither needs it.
fn evaluate(shell: &str) -> &'static str {
    match shell.rsplit('/').next() {
        Some("zsh" | "fish") => "eval",
        _ => "command eval",
    }
}

/// `text` as one word to a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The terminal a pane runs as. The daemon carries its terminfo entry, and the headless
/// terminal answers XTGETTCAP under the same name, so a program that asks gets the same answer
/// both ways.
pub(crate) const TERM: &str = "xterm-ghostty";

/// The Ghostty a pane runs in, as far as a program is concerned: the pinned one (`build.rs`).
const GHOSTTY_VERSION: &str = env!("MUSTER_GHOSTTY_VERSION");

/// A pane's environment: what the daemon inherited, less what names another pane or describes
/// another terminal, plus what the request asked for, plus what the daemon sets itself.
///
/// The pane says it is Ghostty (`TERM_PROGRAM`), because it is a Ghostty terminal and programs
/// key features on that name; `MUSTER_PANE` and `MUSTER_SOCKET` are what say it is Muster's.
/// Its terminfo entry is found through `TERMINFO_DIRS`, ahead of whatever else was there, with an
/// empty entry after it so the system's database is still searched and a person's own
/// `~/.terminfo` still comes first.
pub(crate) fn environment(
    inherited: &[(OsString, OsString)],
    requested: &HashMap<String, String>,
    pane: &str,
    command: Option<&str>,
    terminfo: &Path,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = inherited
        .iter()
        .filter(|(name, _)| !NOT_INHERITED.iter().any(|dropped| name == OsStr::new(dropped)))
        .filter(|(name, _)| !name.to_string_lossy().starts_with(GHOSTTY_PREFIX))
        .cloned()
        .collect();
    let mut requested: Vec<_> = requested.iter().collect();
    requested.sort();
    for (name, value) in requested {
        if name != PANE_NAME && name != PANE_COMMAND {
            put(&mut environment, name, value);
        }
    }
    let mut dirs = OsString::from(terminfo);
    dirs.push(":");
    if let Some((_, existing)) = environment.iter().find(|(name, _)| name == "TERMINFO_DIRS") {
        dirs.push(existing);
    }
    put(&mut environment, "TERMINFO_DIRS", dirs);
    put(&mut environment, "TERM", TERM);
    put(&mut environment, "COLORTERM", "truecolor");
    put(&mut environment, "TERM_PROGRAM", "ghostty");
    put(&mut environment, "TERM_PROGRAM_VERSION", GHOSTTY_VERSION);
    put(&mut environment, PANE_NAME, pane);
    if let Some(command) = command {
        put(&mut environment, PANE_COMMAND, command);
    }
    // Another terminal's claim, which Ghostty drops for the same reason.
    environment.retain(|(name, _)| name != "VTE_VERSION");
    environment
}

/// Sets `name`, replacing any value it had.
fn put(environment: &mut Vec<(OsString, OsString)>, name: &str, value: impl AsRef<OsStr>) {
    environment.retain(|(existing, _)| existing != OsStr::new(name));
    environment.push((name.into(), value.as_ref().into()));
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
        assert_eq!(
            argv("/bin/sh", true, true),
            [
                "/bin/sh",
                "-l",
                "-i",
                "-c",
                "command eval \"$MUSTER_PANE_COMMAND\"\nexec '/bin/sh' -l -i"
            ]
        );
    }

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(name, value)| ((*name).into(), (*value).into())).collect()
    }

    fn sorted(mut environment: Vec<(OsString, OsString)>) -> Vec<(OsString, OsString)> {
        environment.sort();
        environment
    }

    const TERMINFO: &str = "/data/terminfo";

    #[test]
    fn the_pane_is_named_by_the_daemon_and_nothing_else() {
        let inherited = pairs(&[
            ("PATH", "/bin"),
            ("MUSTER_PANE", "p-stale"),
            ("MUSTER_SOCKET", "/stale.sock"),
            ("MUSTER_PANE_COMMAND", "stale"),
            ("COLORTERM", "24bit"),
        ]);
        let requested = HashMap::from([
            ("MUSTER_SOCKET".to_string(), "/window.sock".to_string()),
            ("MUSTER_PANE".to_string(), "p-requested".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        let environment = environment(&inherited, &requested, "p1", None, Path::new(TERMINFO));
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("COLORTERM", "truecolor"),
                ("MUSTER_PANE", "p1"),
                ("MUSTER_SOCKET", "/window.sock"),
                ("PATH", "/usr/bin"),
                ("TERM", "xterm-ghostty"),
                ("TERMINFO_DIRS", "/data/terminfo:"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
            ]))
        );
    }

    #[test]
    fn the_pane_is_a_ghostty_terminal_whatever_it_inherited_or_asked() {
        let inherited = pairs(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("VTE_VERSION", "7600"),
            ("GHOSTTY_RESOURCES_DIR", "/Applications/Ghostty.app/Contents/Resources/ghostty"),
            ("TERMINFO_DIRS", "/opt/terminfo"),
        ]);
        let requested = HashMap::from([
            ("TERM".to_string(), "vt100".to_string()),
            ("GHOSTTY_ASKED".to_string(), "kept".to_string()),
        ]);
        let environment = environment(&inherited, &requested, "p1", None, Path::new(TERMINFO));
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("COLORTERM", "truecolor"),
                ("GHOSTTY_ASKED", "kept"),
                ("MUSTER_PANE", "p1"),
                ("TERM", "xterm-ghostty"),
                ("TERMINFO_DIRS", "/data/terminfo:/opt/terminfo"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
            ]))
        );
    }

    #[test]
    fn the_version_is_the_pinned_ghostty() {
        let pin = include_str!("../../../deps/ghostty.pin").trim();
        let (version, commit) = GHOSTTY_VERSION.split_once('+').unwrap();
        assert_eq!(commit, &pin[..8]);
        assert!(version.starts_with(|c: char| c.is_ascii_digit()), "{version}");
    }
}
