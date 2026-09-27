//! What a new pane runs, and with what environment. Pure: `pty.rs` starts it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::shell_integration::{self, Integration};

/// Variables the daemon sets itself on every pane, which an inherited or requested copy never
/// overrides: a pane must not be told it is some other pane.
const PANE_NAME: &str = "MUSTER_PANE";

/// Where a pane's command travels to its shell (`argv` says why), which the command and
/// everything it starts can also read.
pub(crate) const PANE_COMMAND: &str = "MUSTER_PANE_COMMAND";

/// The daemon's own executable and its socket, so a program in the pane can reach the daemon
/// that owns it with no window involved: `"$MUSTER_DAEMON" report` is how an agent's hooks say
/// what it is doing. The executable rather than a `muster` on `PATH`, because it is the one
/// program certain to be on every machine with panes, a devenv included.
pub(crate) const DAEMON: &str = "MUSTER_DAEMON";
pub(crate) const DAEMON_SOCKET: &str = "MUSTER_DAEMON_SOCKET";

/// How a pane's programs reach the daemon that owns it.
#[derive(Debug, Clone)]
pub(crate) struct Reachable {
    /// Absent when the daemon could not find its own executable.
    pub(crate) daemon: Option<std::path::PathBuf>,
    pub(crate) socket: std::path::PathBuf,
}

/// Variables dropped from what the daemon inherited. A daemon a developer started by hand from
/// inside a Muster pane carries that pane's name, window and command, and a pane that inherited
/// them would drive the wrong window. The requested environment supplies the right
/// `MUSTER_SOCKET`; the daemon supplies the others.
const NOT_INHERITED: [&str; 5] = [PANE_NAME, "MUSTER_SOCKET", PANE_COMMAND, DAEMON, DAEMON_SOCKET];

/// Variables from a Ghostty the daemon was started in - its resources, its binary, its surface -
/// which describe that terminal rather than this pane. A requested copy is still honored.
const GHOSTTY_PREFIX: &str = "GHOSTTY_";

/// What a pane starts: `shell`, with Ghostty's integration for it from `scripts`, running the
/// pane's command first if it has one. `environment` is the pane's, from [`environment`].
pub(crate) fn start(
    shell: &str,
    login: bool,
    runs_command: bool,
    mut environment: Vec<(OsString, OsString)>,
    scripts: &Path,
) -> (Vec<String>, Vec<(OsString, OsString)>) {
    let integration = shell_integration::for_shell(shell, &environment, scripts);
    let argv = argv(shell, login, runs_command, &integration);
    // A command's shell runs it clean; the integration is for the shell it becomes, and the
    // exec line carries it there (`argv`).
    if !runs_command {
        for (name, value) in &integration.environment {
            put(&mut environment, name, value);
        }
    }
    (argv, environment)
}

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
///
/// Only the interactive shell gets the integration. Given to the shell that runs the command,
/// it would be undone before the `exec`: zsh's `.zshenv` and fish's script each take their own
/// injection back out as they load, so the shell exec'd after would start without it, and
/// bash's `ENV` would reach the command and everything it starts. So the `exec` sets the
/// integration's variables itself, through `env`, which every shell here can exec.
fn argv(shell: &str, login: bool, runs_command: bool, integration: &Integration) -> Vec<String> {
    let flags: &[&str] = if login { &["-l", "-i"] } else { &["-i"] };
    let mut argv = vec![shell.to_string()];
    if runs_command {
        argv.extend(flags.iter().map(|flag| (*flag).to_string()));
        let mut exec = vec!["exec".to_string()];
        if !integration.environment.is_empty() {
            exec.push("env".to_string());
            exec.extend(
                integration
                    .environment
                    .iter()
                    .map(|(name, value)| quote(&format!("{name}={value}"))),
            );
        }
        exec.push(quote(shell));
        exec.extend(integration.arguments.iter().cloned());
        exec.extend(flags.iter().map(|flag| (*flag).to_string()));
        argv.push("-c".to_string());
        argv.push(format!("{} \"${PANE_COMMAND}\"\n{}", evaluate(shell), exec.join(" ")));
    } else {
        argv.extend(integration.arguments.iter().cloned());
        argv.extend(flags.iter().map(|flag| (*flag).to_string()));
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

/// Ghostty's default features for its shell integration, which its scripts read, and which it
/// sets whether or not a script was loaded. `sudo` and the `ssh-*` features stay off, as they do
/// in Ghostty, and would need a Ghostty binary if they were on.
const SHELL_FEATURES: &str = "cursor:blink,path,title";

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
    reachable: &Reachable,
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
        if ![PANE_NAME, PANE_COMMAND, DAEMON, DAEMON_SOCKET].contains(&name.as_str()) {
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
    put(&mut environment, "GHOSTTY_SHELL_FEATURES", SHELL_FEATURES);
    put(&mut environment, PANE_NAME, pane);
    if let Some(command) = command {
        put(&mut environment, PANE_COMMAND, command);
    }
    // Another terminal's claim, which Ghostty drops for the same reason.
    environment.retain(|(name, _)| name != "VTE_VERSION");
    if let Some(daemon) = &reachable.daemon {
        put(&mut environment, DAEMON, daemon);
    }
    put(&mut environment, DAEMON_SOCKET, &reachable.socket);
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
        assert_eq!(
            argv("/bin/zsh", true, false, &Integration::default()),
            ["/bin/zsh", "-l", "-i"]
        );
        assert_eq!(argv("/bin/zsh", false, false, &Integration::default()), ["/bin/zsh", "-i"]);
    }

    #[test]
    fn a_command_is_evaluated_and_then_the_shell_becomes_itself() {
        assert_eq!(
            argv("/bin/zsh", true, true, &Integration::default()),
            ["/bin/zsh", "-l", "-i", "-c", "eval \"$MUSTER_PANE_COMMAND\"\nexec '/bin/zsh' -l -i"]
        );
        assert_eq!(
            argv("/opt/it's/fish", false, true, &Integration::default()),
            [
                "/opt/it's/fish",
                "-i",
                "-c",
                "eval \"$MUSTER_PANE_COMMAND\"\nexec '/opt/it'\\''s/fish' -i"
            ]
        );
        assert_eq!(
            argv("/bin/sh", true, true, &Integration::default()),
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

    fn reachable() -> Reachable {
        Reachable {
            daemon: Some("/opt/muster/muster-daemon".into()),
            socket: "/run/daemon.sock".into(),
        }
    }

    #[test]
    fn the_pane_is_named_by_the_daemon_and_nothing_else() {
        let inherited = pairs(&[
            ("PATH", "/bin"),
            ("MUSTER_PANE", "p-stale"),
            ("MUSTER_SOCKET", "/stale.sock"),
            ("MUSTER_PANE_COMMAND", "stale"),
            ("MUSTER_DAEMON_SOCKET", "/stale-daemon.sock"),
            ("COLORTERM", "24bit"),
        ]);
        let requested = HashMap::from([
            ("MUSTER_SOCKET".to_string(), "/window.sock".to_string()),
            ("MUSTER_PANE".to_string(), "p-requested".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("MUSTER_DAEMON".to_string(), "/requested/daemon".to_string()),
        ]);
        let environment =
            environment(&inherited, &requested, "p1", None, Path::new(TERMINFO), &reachable());
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("COLORTERM", "truecolor"),
                ("MUSTER_DAEMON", "/opt/muster/muster-daemon"),
                ("MUSTER_DAEMON_SOCKET", "/run/daemon.sock"),
                ("MUSTER_PANE", "p1"),
                ("MUSTER_SOCKET", "/window.sock"),
                ("PATH", "/usr/bin"),
                ("TERM", "xterm-ghostty"),
                ("TERMINFO_DIRS", "/data/terminfo:"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
                ("GHOSTTY_SHELL_FEATURES", "cursor:blink,path,title"),
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
        let environment =
            environment(&inherited, &requested, "p1", None, Path::new(TERMINFO), &reachable());
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("COLORTERM", "truecolor"),
                ("GHOSTTY_ASKED", "kept"),
                ("MUSTER_DAEMON", "/opt/muster/muster-daemon"),
                ("MUSTER_DAEMON_SOCKET", "/run/daemon.sock"),
                ("MUSTER_PANE", "p1"),
                ("TERM", "xterm-ghostty"),
                ("TERMINFO_DIRS", "/data/terminfo:/opt/terminfo"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
                ("GHOSTTY_SHELL_FEATURES", "cursor:blink,path,title"),
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

    #[test]
    fn a_shell_pane_starts_integrated() {
        let (argv, environment) =
            start("/usr/bin/bash", true, false, pairs(&[("HISTFILE", "/h")]), Path::new("/s"));
        assert_eq!(argv, ["/usr/bin/bash", "--posix", "-l", "-i"]);
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("HISTFILE", "/h"),
                ("ENV", "/s/bash/ghostty.bash"),
                ("GHOSTTY_BASH_INJECT", "1"),
            ]))
        );
    }

    #[test]
    fn a_command_runs_clean_and_the_shell_it_becomes_is_integrated() {
        let (argv, environment) = start("/bin/zsh", true, true, Vec::new(), Path::new("/s"));
        assert_eq!(
            argv,
            [
                "/bin/zsh",
                "-l",
                "-i",
                "-c",
                "eval \"$MUSTER_PANE_COMMAND\"\nexec env 'ZDOTDIR=/s/zsh' '/bin/zsh' -l -i"
            ]
        );
        assert!(environment.is_empty(), "{environment:?}");

        let (argv, _) = start("/bin/bash", false, true, Vec::new(), Path::new("/it's"));
        if !cfg!(target_os = "macos") {
            assert_eq!(
                argv[3],
                "command eval \"$MUSTER_PANE_COMMAND\"\nexec env 'ENV=/it'\\''s/bash/ghostty.bash' \
                 'GHOSTTY_BASH_INJECT=1' '/bin/bash' --posix -i"
            );
        }
    }
}
