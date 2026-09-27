//! What a new pane runs, and with what environment. Pure: `pty.rs` starts it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;

use crate::data::Data;
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
/// program certain to be on every machine with panes, a devenv included. Named through a link
/// beside the socket (`server::point_link`), which follows the pane to whichever daemon holds it
/// after a handoff, so a daemon's directory can go once no daemon runs from it.
pub(crate) const DAEMON: &str = "MUSTER_DAEMON";
pub(crate) const DAEMON_SOCKET: &str = "MUSTER_DAEMON_SOCKET";

/// How a pane's programs reach the daemon that owns it.
#[derive(Debug, Clone)]
pub(crate) struct Reachable {
    /// The link beside the socket, or the executable itself where the link could not be made.
    /// Absent when the daemon could not find its own executable.
    pub(crate) daemon: Option<PathBuf>,
    pub(crate) socket: PathBuf,
}

/// Variables dropped from what the daemon inherited. A daemon a developer started by hand from
/// inside a Muster pane carries that pane's name, window and command, and a pane that inherited
/// them would drive the wrong window. The requested environment supplies the right
/// `MUSTER_SOCKET`; the daemon supplies the others. `MUSTER_LOG_FILE` names the log of the run
/// that started the daemon, which the daemon outlives: a `muster` run in a pane would write
/// into that run's file, so only a pane's create may name one.
const NOT_INHERITED: [&str; 6] =
    [PANE_NAME, "MUSTER_SOCKET", PANE_COMMAND, DAEMON, DAEMON_SOCKET, "MUSTER_LOG_FILE"];

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
/// sh, bash, zsh and fish - but see [`Language::evaluate`] for how each is asked.
///
/// Only the interactive shell gets the integration. Given to the shell that runs the command,
/// it would be undone before the `exec`: zsh's `.zshenv` and fish's script each take their own
/// injection back out as they load, so the shell exec'd after would start without it, and
/// bash's `ENV` would reach the command and everything it starts. So the shell that ran the
/// command exports the integration's variables itself just before its `exec`, rather than
/// handing them to `env`: `env` would be found through a `PATH` the command may have changed,
/// and reads any argument containing `=` as another variable, a shell's path included.
fn argv(shell: &str, login: bool, runs_command: bool, integration: &Integration) -> Vec<String> {
    let flags: &[&str] = if login { &["-l", "-i"] } else { &["-i"] };
    let mut argv = vec![shell.to_string()];
    if runs_command {
        let language = Language::of(shell);
        argv.extend(flags.iter().map(|flag| (*flag).to_string()));
        let mut script = vec![format!("{} \"${PANE_COMMAND}\"", language.evaluate())];
        script.extend(
            integration.environment.iter().map(|(name, value)| language.export(name, value)),
        );
        let mut exec = vec!["exec".to_string(), language.quote(shell)];
        exec.extend(integration.arguments.iter().cloned());
        exec.extend(flags.iter().map(|flag| (*flag).to_string()));
        script.push(exec.join(" "));
        argv.push("-c".to_string());
        argv.push(script.join("\n"));
    } else {
        argv.extend(integration.arguments.iter().cloned());
        argv.extend(flags.iter().map(|flag| (*flag).to_string()));
    }
    argv
}

/// Which language the exec line is written in, from the shell's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Posix,
    Zsh,
    Fish,
}

impl Language {
    fn of(shell: &str) -> Language {
        match shell.rsplit('/').next() {
            Some("zsh") => Language::Zsh,
            Some("fish") => Language::Fish,
            _ => Language::Posix,
        }
    }

    /// How the command is evaluated.
    ///
    /// `eval` is a special builtin in a POSIX shell, and a syntax error inside one abandons the
    /// rest of the script: dash, Debian's and Ubuntu's `/bin/sh`, then never reaches the `exec`,
    /// and the pane is left in the shell that ran the command rather than a login shell of its
    /// own. `command` takes the special status away, so the error is an ordinary failure and the
    /// script goes on. zsh and fish read `command eval` as an external program called `eval`, and
    /// neither needs it.
    fn evaluate(self) -> &'static str {
        match self {
            Language::Posix => "command eval",
            Language::Zsh | Language::Fish => "eval",
        }
    }

    /// `text` as one word. fish's single quotes take `\\` and `\'` as escapes, where a POSIX
    /// shell's take nothing, so a value ending in a backslash would leave fish's string open.
    fn quote(self, text: &str) -> String {
        match self {
            Language::Posix | Language::Zsh => format!("'{}'", text.replace('\'', r"'\''")),
            Language::Fish => format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'")),
        }
    }

    /// Exports `name` to what the shell execs. `name` is one of the integration's own, never
    /// anything a request supplied, so it needs no quoting.
    fn export(self, name: &str, value: &str) -> String {
        match self {
            Language::Posix | Language::Zsh => format!("export {name}={}", self.quote(value)),
            Language::Fish => format!("set -gx {name} {}", self.quote(value)),
        }
    }
}

/// The terminal a pane runs as. The daemon carries its terminfo entry, and the headless
/// terminal answers XTGETTCAP under the same name, so a program that asks gets the same answer
/// both ways.
pub(crate) const TERM: &str = "xterm-ghostty";

/// The Ghostty a pane runs in, as far as a program is concerned: the pinned one (`build.rs`).
/// XTVERSION answers with it too (`screen.rs`), so the two agree.
pub(crate) const GHOSTTY_VERSION: &str = env!("MUSTER_GHOSTTY_VERSION");

/// The features Ghostty's shell integration is told to use, which its scripts read, and which
/// Ghostty sets whether or not a script was loaded, each as the settings say (`proto::Shell`).
/// `ssh-terminfo` and `ssh-env` (on unless set off) wrap `ssh` so that the host it reaches is
/// given the entry and the terminal's name, through `$GHOSTTY_BIN_DIR/ghostty +ssh`, which is
/// Muster's stand-in (`data.rs`). `sudo` (off unless set on) wraps `sudo` so that it keeps
/// `$TERMINFO`, which sudo's reset environment would drop; that needs a sudoers rule allowing
/// SETENV, and breaks sudo under one that does not, which is why it is off. `path`, which would
/// put that directory on the PATH, stays off: it holds no Ghostty.
///
/// `cursor` makes every prompt set a bar cursor, blinking or steady as `cursor-style-blink` is,
/// which is Ghostty's rule and applies here while the app's `[cursor]` names no shape. A shape it
/// names is left alone at the prompt too: Muster turns the integration on without being asked,
/// so the person asked for that shape and never for a bar. With no settings from an app yet, as
/// in a daemon no window has reached, this is Ghostty's default, a blinking bar.
fn shell_features(cursor: Option<&proto::Cursor>, shell: &proto::Shell) -> String {
    let named = cursor.is_some_and(|cursor| cursor.style() != proto::CursorStyle::Unspecified);
    let blink = cursor.and_then(|cursor| cursor.blink).unwrap_or(true);
    let mut features = Vec::new();
    if !named {
        features.push(if blink { "cursor:blink" } else { "cursor:steady" });
    }
    if shell.ssh_env.unwrap_or(true) {
        features.push("ssh-env");
    }
    if shell.ssh_terminfo.unwrap_or(true) {
        features.push("ssh-terminfo");
    }
    if shell.sudo.unwrap_or(false) {
        features.push("sudo");
    }
    features.push("title");
    features.join(",")
}

/// The settings a pane's environment follows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settings<'a> {
    pub(crate) shell: &'a proto::Shell,
    pub(crate) cursor: Option<&'a proto::Cursor>,
}

/// A pane's environment: what the daemon inherited, less what names another pane or describes
/// another terminal, plus what the request asked for, plus what the daemon sets itself.
///
/// The pane says it is Ghostty (`TERM_PROGRAM`), because it is a Ghostty terminal and programs
/// key features on that name; `MUSTER_PANE` and `MUSTER_SOCKET` are what say it is Muster's.
/// Its terminfo entry is the daemon's, through `TERMINFO_DIRS`, ahead of whatever else was
/// there and with an empty entry after it so the system's database is still searched.
/// `TERMINFO` is not the daemon's data directory, as it is Ghostty.app's in Ghostty's panes,
/// because tic writes into `TERMINFO` when it can, and the daemon's data can be a signed bundle.
/// With the `sudo` feature it is the pane's `~/.terminfo`, which sudo then carries, and which
/// [`put_entry_in_home`] gives the entry; otherwise it is unset, so one inherited from another
/// terminal does not decide which entry the pane gets.
pub(crate) fn environment(
    inherited: &[(OsString, OsString)],
    requested: &HashMap<String, String>,
    pane: &str,
    command: Option<&str>,
    data: &Data,
    reachable: &Reachable,
    settings: Settings<'_>,
) -> Vec<(OsString, OsString)> {
    let Settings { shell, cursor } = settings;
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
    let existing = environment.iter().find(|(name, _)| name == "TERMINFO_DIRS");
    let terminfo = data.terminfo();
    let dirs = terminfo_dirs(&terminfo, existing.map(|(_, dirs)| dirs.as_os_str()));
    put(&mut environment, "TERMINFO_DIRS", dirs);
    put(&mut environment, "TERM", TERM);
    put(&mut environment, "COLORTERM", "truecolor");
    put(&mut environment, "TERM_PROGRAM", "ghostty");
    put(&mut environment, "TERM_PROGRAM_VERSION", GHOSTTY_VERSION);
    put(&mut environment, "GHOSTTY_SHELL_FEATURES", shell_features(cursor, shell));
    put(&mut environment, PANE_NAME, pane);
    if let Some(command) = command {
        put(&mut environment, PANE_COMMAND, command);
    }
    // Another terminal's claim, which Ghostty drops for the same reason.
    environment.retain(|(name, _)| name != "VTE_VERSION");
    environment.retain(|(name, _)| name != "TERMINFO");
    if shell.sudo.unwrap_or(false)
        && let Some(home) = home_terminfo(&environment)
    {
        put(&mut environment, "TERMINFO", home);
    }
    put(&mut environment, "GHOSTTY_BIN_DIR", data.bin());
    if let Some(daemon) = &reachable.daemon {
        put(&mut environment, DAEMON, daemon);
    }
    put(&mut environment, DAEMON_SOCKET, &reachable.socket);
    environment
}

/// `~/.terminfo` for the environment's `HOME`.
fn home_terminfo(environment: &[(OsString, OsString)]) -> Option<PathBuf> {
    let (_, home) = environment.iter().find(|(name, _)| name == "HOME")?;
    Some(Path::new(home).join(".terminfo"))
}

/// Copies the daemon's compiled entries into `~/.terminfo` where it has none of the same name,
/// for the `sudo` feature, which has root read the pane's `TERMINFO`. An entry already there is
/// the person's, and stays: `~/.terminfo` is theirs to keep current.
pub(crate) fn put_entry_in_home(environment: &[(OsString, OsString)], data: &Data) {
    let Some(home) = home_terminfo(environment) else { return };
    let Ok(letters) = std::fs::read_dir(data.terminfo()) else { return };
    for letter in letters.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(entries) = std::fs::read_dir(letter.path()) else { continue };
        for entry in entries.flatten() {
            let target = home.join(letter.file_name()).join(entry.file_name());
            if target.exists() {
                continue;
            }
            let copied = std::fs::create_dir_all(target.parent().unwrap_or(&home))
                .and_then(|()| std::fs::copy(entry.path(), &target));
            if let Err(error) = copied {
                log::warn(
                    "daemon.spawn.terminfo_not_copied",
                    fields! {
                        "to" => target.display(),
                        "error" => error,
                        "impact" => "root, through sudo, may not find this terminal's terminfo entry",
                        "check" => "that ~/.terminfo is writable, or turn [shell] sudo off",
                    },
                );
            }
        }
    }
}

/// The daemon's terminfo directory, then `existing`, then an empty entry, which ncurses reads as
/// its own compiled-in database. Without one, setting `TERMINFO_DIRS` at all would stop the
/// system's entries being found. An `existing` list that already has one keeps its own place.
fn terminfo_dirs(terminfo: &Path, existing: Option<&OsStr>) -> OsString {
    let mut dirs = OsString::from(terminfo);
    dirs.push(":");
    if let Some(existing) = existing.filter(|existing| !existing.is_empty()) {
        dirs.push(existing);
        if !existing.to_string_lossy().split(':').any(str::is_empty) {
            dirs.push(":");
        }
    }
    dirs
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
                "eval \"$MUSTER_PANE_COMMAND\"\nexec '/opt/it\\'s/fish' -i"
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

    const DATA: &str = "/data";

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
            ("MUSTER_LOG_FILE", "/stale-run.jsonl"),
            ("COLORTERM", "24bit"),
        ]);
        let requested = HashMap::from([
            ("MUSTER_SOCKET".to_string(), "/window.sock".to_string()),
            ("MUSTER_PANE".to_string(), "p-requested".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("MUSTER_DAEMON".to_string(), "/requested/daemon".to_string()),
        ]);
        let environment = environment(
            &inherited,
            &requested,
            "p1",
            None,
            &Data::unchecked(PathBuf::from(DATA)),
            &reachable(),
            Settings { shell: &proto::Shell::default(), cursor: None },
        );
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
                ("GHOSTTY_BIN_DIR", "/data/bin"),
                ("TERMINFO_DIRS", "/data/terminfo:"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
                ("GHOSTTY_SHELL_FEATURES", "cursor:blink,ssh-env,ssh-terminfo,title"),
            ]))
        );
    }

    /// `sudo` wraps sudo to keep `TERMINFO`, which is then the pane's `~/.terminfo`: where the
    /// daemon puts the entry, and where tic writes, rather than the daemon's own data directory.
    /// The ssh features follow their settings.
    #[test]
    fn the_shell_features_follow_the_settings() {
        let inherited = pairs(&[("HOME", "/home/me"), ("TERMINFO", "/elsewhere")]);
        let shell = |ssh_env, ssh_terminfo, sudo| proto::Shell {
            ssh_env: Some(ssh_env),
            ssh_terminfo: Some(ssh_terminfo),
            sudo: Some(sudo),
            ..proto::Shell::default()
        };
        let data = Data::unchecked(PathBuf::from(DATA));
        let find = |environment: &[(OsString, OsString)], name: &str| {
            environment
                .iter()
                .find(|(found, _)| found == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        let with = |shell: &proto::Shell| {
            environment(
                &inherited,
                &HashMap::new(),
                "p1",
                None,
                &data,
                &reachable(),
                Settings { shell, cursor: None },
            )
        };

        let sudo = with(&shell(true, true, true));
        assert_eq!(
            find(&sudo, "GHOSTTY_SHELL_FEATURES").as_deref(),
            Some("cursor:blink,ssh-env,ssh-terminfo,sudo,title")
        );
        assert_eq!(find(&sudo, "TERMINFO").as_deref(), Some("/home/me/.terminfo"));

        let none = with(&shell(false, false, false));
        assert_eq!(find(&none, "GHOSTTY_SHELL_FEATURES").as_deref(), Some("cursor:blink,title"));
        assert_eq!(find(&none, "TERMINFO"), None, "an inherited TERMINFO would pick the entry");
    }

    #[test]
    fn the_pane_is_a_ghostty_terminal_whatever_it_inherited_or_asked() {
        let inherited = pairs(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal"),
            ("VTE_VERSION", "7600"),
            ("GHOSTTY_RESOURCES_DIR", "/Applications/Ghostty.app/Contents/Resources/ghostty"),
            ("TERMINFO_DIRS", "/opt/terminfo"),
            ("TERMINFO", "/Applications/Ghostty.app/Contents/Resources/terminfo"),
        ]);
        let requested = HashMap::from([
            ("TERM".to_string(), "vt100".to_string()),
            ("GHOSTTY_ASKED".to_string(), "kept".to_string()),
            ("TERMINFO".to_string(), "/mine".to_string()),
        ]);
        let environment = environment(
            &inherited,
            &requested,
            "p1",
            None,
            &Data::unchecked(PathBuf::from(DATA)),
            &reachable(),
            Settings { shell: &proto::Shell::default(), cursor: None },
        );
        assert_eq!(
            sorted(environment),
            sorted(pairs(&[
                ("COLORTERM", "truecolor"),
                ("GHOSTTY_ASKED", "kept"),
                ("MUSTER_DAEMON", "/opt/muster/muster-daemon"),
                ("MUSTER_DAEMON_SOCKET", "/run/daemon.sock"),
                ("MUSTER_PANE", "p1"),
                ("TERM", "xterm-ghostty"),
                ("GHOSTTY_BIN_DIR", "/data/bin"),
                ("TERMINFO_DIRS", "/data/terminfo:/opt/terminfo:"),
                ("TERM_PROGRAM", "ghostty"),
                ("TERM_PROGRAM_VERSION", GHOSTTY_VERSION),
                ("GHOSTTY_SHELL_FEATURES", "cursor:blink,ssh-env,ssh-terminfo,title"),
            ]))
        );
    }

    #[test]
    fn the_prompt_cursor_follows_the_apps_cursor_as_ghostty_does() {
        let cursor = |style, blink| proto::Cursor { style: style as i32, blink };
        let unnamed = proto::CursorStyle::Unspecified;
        let default = proto::Shell::default();
        assert_eq!(shell_features(None, &default), "cursor:blink,ssh-env,ssh-terminfo,title");
        assert_eq!(
            shell_features(Some(&cursor(unnamed, None)), &default),
            "cursor:blink,ssh-env,ssh-terminfo,title"
        );
        assert_eq!(
            shell_features(Some(&cursor(unnamed, Some(false))), &default),
            "cursor:steady,ssh-env,ssh-terminfo,title"
        );
        let block = cursor(proto::CursorStyle::Block, Some(false));
        assert_eq!(shell_features(Some(&block), &default), "ssh-env,ssh-terminfo,title");
        assert_eq!(
            shell_features(Some(&cursor(proto::CursorStyle::Bar, None)), &default),
            "ssh-env,ssh-terminfo,title"
        );
    }

    #[test]
    fn the_systems_terminfo_is_still_searched_after_the_daemons() {
        let dirs = |existing: Option<&str>| {
            terminfo_dirs(Path::new("/data/terminfo"), existing.map(OsStr::new))
                .into_string()
                .unwrap()
        };
        assert_eq!(dirs(None), "/data/terminfo:");
        assert_eq!(dirs(Some("")), "/data/terminfo:");
        assert_eq!(dirs(Some("/opt/terminfo")), "/data/terminfo:/opt/terminfo:");
        assert_eq!(dirs(Some("/a::/b")), "/data/terminfo:/a::/b");
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
                "eval \"$MUSTER_PANE_COMMAND\"\nexport ZDOTDIR='/s/zsh'\nexec '/bin/zsh' -l -i"
            ]
        );
        assert!(environment.is_empty(), "{environment:?}");

        let (argv, _) = start("/bin/bash", false, true, Vec::new(), Path::new("/it's"));
        if !cfg!(target_os = "macos") {
            assert_eq!(
                argv[3],
                "command eval \"$MUSTER_PANE_COMMAND\"\n\
                 export ENV='/it'\\''s/bash/ghostty.bash'\n\
                 export GHOSTTY_BASH_INJECT='1'\n\
                 exec '/bin/bash' --posix -i"
            );
        }
    }

    /// `env` read any argument containing `=` as a variable, a shell's path included, so the
    /// exec line exports the integration's variables itself and execs the shell by its path.
    #[test]
    fn a_shell_whose_path_has_an_equals_sign_is_still_the_shell_execd() {
        let (argv, _) = start("/opt/a=b/zsh", false, true, Vec::new(), Path::new("/s"));
        assert_eq!(
            argv[3],
            "eval \"$MUSTER_PANE_COMMAND\"\nexport ZDOTDIR='/s/zsh'\nexec '/opt/a=b/zsh' -i"
        );
    }

    #[test]
    fn fish_is_written_to_in_fish() {
        let (argv, _) = start("/usr/bin/fish", false, true, Vec::new(), Path::new(r"/it's\"));
        assert_eq!(
            argv[3],
            "eval \"$MUSTER_PANE_COMMAND\"\n\
             set -gx GHOSTTY_SHELL_INTEGRATION_XDG_DIR '/it\\'s\\\\'\n\
             set -gx XDG_DATA_DIRS '/it\\'s\\\\:/usr/local/share:/usr/share'\n\
             exec '/usr/bin/fish' -i"
        );
    }

    /// The same line, read by a real fish: it parses, and the value it exports is the one given.
    /// Skipped where fish is not installed, which a Mac without Homebrew's fish and CI both are.
    #[test]
    fn fish_reads_back_the_value_it_was_given() {
        let data = r"/it's\";
        let (argv, _) = start("/usr/bin/fish", false, true, Vec::new(), Path::new(data));
        let line = &argv[3];
        let fish = |script: &str, check_only: bool| {
            let mut command = std::process::Command::new("fish");
            if check_only {
                command.arg("--no-execute");
            }
            command.arg("-c").arg(script).output()
        };
        let Ok(parsed) = fish(line, true) else {
            eprintln!("fish is not installed here, so its reading of the exec line goes unchecked");
            return;
        };
        assert!(parsed.status.success(), "{}", String::from_utf8_lossy(&parsed.stderr));

        let exports: Vec<&str> = line.lines().filter(|line| line.starts_with("set -gx")).collect();
        let echoed = fish(
            &format!("{}\nprintf '%s' \"$GHOSTTY_SHELL_INTEGRATION_XDG_DIR\"", exports.join("\n")),
            false,
        )
        .expect("fish ran once already");
        assert_eq!(String::from_utf8_lossy(&echoed.stdout), data);
    }
}
