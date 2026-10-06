//! What a daemon is started with: an allowlist of what Muster was handed, and what Muster adds.
//!
//! A daemon outlives the app, the window and the shell that started it, and every pane it ever
//! spawns is its child, so whatever it is handed at birth becomes permanent state handed to every
//! agent. Pure: the caller reads the environment, so every rule here is answerable to
//! `corpus/conformance/daemon-environment.json`.

use std::collections::BTreeMap;

/// What a pane reads to find out which pane it is. The daemon sets it from the name in the
/// request that made the pane; renaming it breaks every pane already running.
pub const PANE_NAME: &str = "MUSTER_PANE";

/// What a pane reads to find the window it is in, which the window sets on every pane it makes.
///
/// The pair with [`PANE_NAME`], and useless without it: knowing which Muster to ask is half of
/// being able to say "this pane". Set per window rather than looked up, because a machine can
/// have several Musters open and a pane belongs to exactly one.
pub const WINDOW_SOCKET: &str = "MUSTER_SOCKET";

/// The whole environment to start this machine's daemon with: what it may carry from
/// `environment`, then what Muster supplies on top.
pub fn for_daemon(
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    commands: Option<&str>,
) -> BTreeMap<String, String> {
    let mut given = carried(environment);
    given.extend(supplied(environment, locale, commands));
    given
}

/// The whole environment to start a daemon on another machine with, from that machine's own
/// environment as an ssh session there sees it.
///
/// The same allowlist, less `SSH_AUTH_SOCK`. Over there it names the agent forwarding of the one
/// ssh connection that happened to start the daemon, a path unique to that connection, so it
/// goes stale for every pane the moment the connection ends. The rest of an ssh session's own
/// variables (`SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`) were never on the list. Nothing is
/// supplied: the locale and the command directory are this machine's answers, not that one's.
pub fn for_far_daemon(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut given = carried(environment);
    given.remove("SSH_AUTH_SOCK");
    given
}

/// The whole environment to start this machine's daemon with from a shell, which is what the
/// `muster` CLI does when a `muster msg` verb finds none running.
///
/// [`for_daemon`] with no platform locale, which only the app's shell can ask for, and less
/// `SSH_AUTH_SOCK` when the shell is an ssh session's: there it names that one connection's
/// agent forwarding, which goes stale for every pane once the connection ends, the reason
/// [`for_far_daemon`] drops it. A pane's own `SSH_AUTH_SOCK` came from its daemon and is kept.
pub fn for_daemon_from_a_shell(
    environment: &BTreeMap<String, String>,
    commands: Option<&str>,
) -> BTreeMap<String, String> {
    let mut given = for_daemon(environment, None, commands);
    if environment.get("SSH_CONNECTION").is_some_and(|connection| !connection.is_empty()) {
        given.remove("SSH_AUTH_SOCK");
    }
    given
}

/// What a daemon is entitled to inherit from whoever launched Muster.
///
/// An allowlist, because a denylist has to keep up with every tool that invents a variable and
/// is wrong until somebody notices it is. The consequence of being wrong is not a broken
/// launch: it is a daemon that outlives the app, carrying one session's private state into
/// every agent it ever spawns. Observed rather than imagined - launching Muster from inside a
/// Claude Code session put that session's `CLAUDE_CODE_*` markers and messaging credentials
/// into the daemon, and from there into every pane, where a fresh Claude Code read them and
/// silently turned its own transcript saving off.
///
/// **The list is short because a pane runs a shell, and a shell builds its own world.** Login
/// shells re-read the user's rc files inside the pane, so everything a toolchain manager,
/// language version switcher or prompt puts in the environment is rebuilt there. What has to
/// survive is only what a shell cannot work out for itself: where home is, what to run, and
/// what the machine's conventions are.
pub fn carried(environment: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    environment
        .iter()
        .filter(|(name, value)| !value.is_empty() && is_carried(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn is_carried(name: &str) -> bool {
    // Locale comes as a family - LC_ALL, LC_CTYPE, LC_TIME and the rest - and carrying some of
    // it is worse than carrying none: a pane with LANG set and LC_CTYPE not renders wide
    // glyphs differently from the terminal it was launched from.
    name.starts_with("LC_") || CARRIED.contains(&name)
}

/// What Muster gives its daemon that nobody handed Muster.
///
/// The other half of [`carried`], and it exists because an allowlist can only carry what is
/// there. A window launched the way Muster is meant to be launched - Dock, Finder, Spotlight -
/// is started by launchd, which hands a GUI process `HOME`, `PATH`, `SHELL`, `USER`,
/// `LOGNAME`, `TMPDIR` and little else. No `LANG`, no `LC_*`.
///
/// **Today a daemon gets one anyway, and that is the reason this exists rather than evidence
/// that it need not.** `ghostty_init` calls Ghostty's own `ensureLocale`, which derives a
/// locale from `CFLocale` and `setenv`s it into the whole process - so by the time Muster
/// starts a daemon the environment it reads has a `LANG` in it that no shell put there.
/// Measured: a bundle opened under `env -i` gives its daemon `LANG=en_AU.UTF-8` and a
/// `LANGUAGE` beside it, which is Ghostty's pair and nothing else's. That is a loan: it is
/// invisible from here, it depends on the renderer being built before the daemon is started,
/// and it is the day a renderer changes that every pane silently drops to the C locale.
///
/// So Muster answers the question itself. `locale` is what the platform said, which only the
/// shell can ask. Whether a daemon gets it is decided here, and only when the environment
/// names *nothing* in the locale family: a `LANG` supplied beside an inherited `LC_CTYPE` is
/// the split locale [`is_carried`] already refuses to create, arrived at from the other
/// direction.
pub fn supplied(
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    commands: Option<&str>,
) -> BTreeMap<String, String> {
    let mut supplied = BTreeMap::new();
    if let Some(locale) = locale.filter(|locale| !locale.is_empty())
        && !names_a_locale(environment)
    {
        supplied.insert("LANG".to_string(), locale.to_string());
    }
    if let Some(path) = commands
        .filter(|path| !path.is_empty())
        .and_then(|commands| with_commands(environment, commands))
    {
        supplied.insert("PATH".to_string(), path);
    }
    supplied
}

/// `PATH` with Muster's own command directory in front of it.
///
/// The one entry here that is Muster's, on a variable that was inherited - so `PATH` ends up in
/// both this list and [`carried`], which is the honest description of a value that was handed over
/// and then added to. It is in this half because this is the list somebody reads when `muster` is
/// not found in a pane.
///
/// In front rather than behind, so a pane reaches the CLI belonging to the window it is drawn in
/// rather than one somebody installed years ago and forgot. macOS `path_helper` appends to an
/// inherited PATH rather than replacing it, so the entry survives a pane's login shell.
///
/// None when there is nothing to do. Already on the PATH is the common case for anybody who put
/// the directory in their own profile, and adding it again would lengthen the PATH of every daemon
/// Muster ever starts. An *empty* PATH is left empty on purpose: a one-entry PATH holding only
/// Muster's commands is a pane whose shell cannot run `ls`, which is worse than a pane with no
/// `muster` in it.
fn with_commands(environment: &BTreeMap<String, String>, commands: &str) -> Option<String> {
    let path = environment.get("PATH").filter(|path| !path.is_empty())?;
    if path.split(':').any(|entry| entry == commands) {
        return None;
    }
    Some(format!("{commands}:{path}"))
}

/// Whether anything in this environment already decides what the locale is.
fn names_a_locale(environment: &BTreeMap<String, String>) -> bool {
    environment
        .iter()
        .any(|(name, value)| !value.is_empty() && (name == "LANG" || name.starts_with("LC_")))
}

/// The variables a daemon carries, and why each one is here.
///
/// Anything not on this list is a variable a pane's own shell can rebuild, or one that
/// belonged to whoever launched Muster and not to the agents Muster runs.
const CARRIED: &[&str] = &[
    // Where home is, and where the tools a pane runs keep their config, state and caches.
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    // What to run in a pane, and what it needs to find anything. A daemon with no PATH spawns
    // a shell that cannot run `ls`.
    "PATH",
    "SHELL",
    // Who the person is. Tools that look up a home directory or a git author read these, and
    // a shell cannot invent them.
    "USER",
    "LOGNAME",
    // The machine's conventions. Wrong or missing, and a pane mangles non-ASCII or writes
    // scratch files somewhere unexpected. A launch that supplies no locale at all gets one
    // anyway - see `supplied`.
    "LANG",
    "TZ",
    "TMPDIR",
    // TERM is deliberately absent, and this note is the whole reason to look for it here.
    //
    // No pane ever sees the daemon's: the daemon sets TERM, COLORTERM and TERM_PROGRAM on every
    // pane itself, because a pane is a Ghostty terminal whatever launched the app. Dropping it
    // also gives the daemon the same environment whether Muster was started from a terminal or
    // from the Dock, which never hands it one.
    //
    // The user's own ssh agent. A deliberate inclusion rather than an oversight: this is a
    // credential channel, and a pane that cannot `git push` or reach a devenv is a pane
    // somebody stops using. It is the person's own agent, it is what every terminal emulator
    // on this platform passes through, and unlike a harness's session token it belongs to the
    // human rather than to whichever program happened to launch Muster. A daemon on another
    // machine does not carry it (`for_far_daemon`).
    "SSH_AUTH_SOCK",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect()
    }

    #[test]
    fn a_shell_keeps_its_own_ssh_agent_and_drops_an_ssh_sessions() {
        let at_the_machine = environment(&[("HOME", "/h"), ("SSH_AUTH_SOCK", "/agent")]);
        assert_eq!(
            for_daemon_from_a_shell(&at_the_machine, None).get("SSH_AUTH_SOCK").map(String::as_str),
            Some("/agent")
        );
        let over_ssh = environment(&[
            ("HOME", "/h"),
            ("SSH_AUTH_SOCK", "/tmp/ssh-x/agent.1"),
            ("SSH_CONNECTION", "10.0.0.1 5000 10.0.0.2 22"),
        ]);
        let given = for_daemon_from_a_shell(&over_ssh, None);
        assert_eq!(given.get("SSH_AUTH_SOCK"), None);
        assert_eq!(given.get("HOME").map(String::as_str), Some("/h"));
        assert_eq!(given.get("SSH_CONNECTION"), None);
    }

    #[test]
    fn a_shell_puts_the_commands_directory_in_front_and_supplies_no_locale() {
        let given = for_daemon_from_a_shell(
            &environment(&[("PATH", "/usr/bin:/bin")]),
            Some("/h/.muster/bin"),
        );
        assert_eq!(given.get("PATH").map(String::as_str), Some("/h/.muster/bin:/usr/bin:/bin"));
        assert_eq!(given.get("LANG"), None);
    }
}
