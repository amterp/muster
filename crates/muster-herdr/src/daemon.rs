//! Muster's own daemon: whether one is listening, and starting one when none is.
//!
//! Muster ships a herdr and runs it rather than asking anybody to install one, because a
//! person using Muster should not have to learn what herdr is. That only means something if
//! the daemon it talks to is the daemon it shipped: an arbitrary one on the default socket is
//! an arbitrary version, and the corpus this project is judged against says nothing about
//! versions it was not recorded from. So Muster runs its own under a herdr session of its own
//! ([`crate::discovery::OWN_SESSION`]) and never meets a stranger.
//!
//! Started, never stopped. Sessions outliving the app is a founding desideratum, so the
//! daemon is put in its own process group and quitting Muster costs it nothing - the whole
//! point is that the agents keep working.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use serde_json::json;

use crate::client::HerdrClient;
use crate::discovery::{OWN_SESSION, config_file};

/// How long a daemon that has just been started gets to answer.
///
/// Generous, because the alternative is worse: a launch that gives up early reports "no
/// daemon" about a daemon that is seconds from being ready, and the window it produces is the
/// empty one this whole path exists to prevent. A healthy start answers in well under a
/// second, so nobody who is not already broken waits this long.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait on a socket that should already have a daemon behind it.
///
/// Short, because this runs on every launch and the answer is nearly always immediate.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Whether something is listening on this socket and answering herdr's protocol.
///
/// A `ping` rather than a file check: a socket file outlives the daemon that made it, so its
/// presence is not an answer. A stale one is the ordinary state after a crash or a reboot.
pub fn answers(socket_path: &str) -> bool {
    HerdrClient::with_timeout(socket_path, PROBE_TIMEOUT).request("ping", &json!({})).is_ok()
}

/// Starts a daemon on this socket and waits until it answers.
///
/// The environment is built from an allowlist rather than inherited, and that is the whole of
/// [`carried`]. Everything the launching shell held would otherwise become permanent state in
/// a process that outlives the app and hands it to every pane it ever spawns.
///
/// What launchd did not give a GUI process is put back, and that is the whole of [`supplied`].
///
/// `HERDR_SESSION` rather than `--session`, so it reaches the panes the daemon spawns too.
/// That is what makes `herdr pane list` inside a Muster pane talk to the daemon that owns it
/// rather than to whatever the user's own herdr would find.
pub fn start(
    binary: &str,
    socket_path: &str,
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> Result<(), String> {
    log::info(
        "daemon.starting",
        fields! {
            "binary" => binary,
            "socket" => socket_path,
            "session" => OWN_SESSION,
            // Whose config decides what a pane runs, which is the first question when a pane
            // opens the wrong shell. Muster's own derived file where there is one, and the
            // user's herdr config where the shell named nowhere to write one.
            "config" => config_path
                .map(ToString::to_string)
                .or_else(|| config_file(environment))
                .unwrap_or_default(),
        },
    );

    let carried = carried(environment);
    let supplied = supplied(environment, locale, config_path, commands);
    let dropped: Vec<&str> = environment
        .keys()
        .filter(|name| !carried.contains_key(*name))
        .map(String::as_str)
        .collect();
    // Names, never values: this log is meant to be attachable to a bug report, and an
    // environment holds tokens (`architecture.md`, the diagnostic log). The names are what
    // somebody asking "why does my pane not see FOO" needs, and they are not secrets.
    //
    // Three lists rather than two, because a supplied variable is neither carried nor
    // dropped - it was never in the environment to be either. Reading a Dock launch's log
    // without it, the honest question "where did LANG come from" has no answer in the file.
    log::info(
        "daemon.environment",
        fields! {
            "carried" => carried.keys().cloned().collect::<Vec<String>>().join(","),
            "supplied" => supplied.keys().cloned().collect::<Vec<String>>().join(","),
            "dropped_count" => dropped.len().to_string(),
            "dropped" => dropped.join(","),
        },
    );

    let mut child = Command::new(binary)
        .arg("server")
        .env_clear()
        .envs(&carried)
        .envs(&supplied)
        .env("HERDR_SESSION", OWN_SESSION)
        // Its own process group, so that Muster quitting - or being killed with the terminal
        // it was launched from - does not take the agents with it.
        .process_group(0)
        .stdin(Stdio::null())
        // Its startup banner names the socket and the log it just opened, which is Muster's
        // job to report rather than herdr's. Standard error is left inherited: it carries the
        // failures that happen before herdr can open a log of its own - a socket path over
        // the `sockaddr_un` limit, a binary for the wrong architecture - and a daemon that
        // never started has nowhere else to say so.
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| {
            format!(
                "the daemon at {binary} could not be started ({error}). Muster ships this \
                 binary inside its own bundle, so a missing or unrunnable one usually means a \
                 build that never staged it - `./dev -b` puts it beside the app - or a \
                 MUSTER_HERDR pointing somewhere stale."
            )
        })?;

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if answers(socket_path) {
            log::info("daemon.started", fields! { "socket" => socket_path });
            return Ok(());
        }
        // A daemon that exited says so now rather than at the end of the timeout, and its
        // status is the only clue there is - it has written no log if it never bound.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the daemon exited with {status} before it accepted a connection, so this \
                 window has no session behind it. A socket path over the ~104 bytes a Unix \
                 socket allows and a binary for the wrong architecture both look like this; \
                 {binary} run by hand says which."
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "the daemon started but did not answer on {socket_path} within {}s, so this window \
         has no session behind it. It may still be coming up, in which case relaunching \
         Muster will find it.",
        START_TIMEOUT.as_secs()
    ))
}

/// Ensures a daemon is listening on this socket, starting Muster's own if none is.
///
/// The order is deliberate: ask first, start second. A daemon left running by an earlier
/// Muster is exactly what should be reused - that is the whole of "sessions outlive the app" -
/// and starting a second one would bind nothing and lose the first one's panes.
pub fn ensure_running(
    socket_path: &str,
    binary: Option<&str>,
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> Result<Reached, String> {
    if answers(socket_path) {
        return Ok(Reached::Adopted);
    }
    let Some(binary) = binary else {
        return Err(format!(
            "nothing is listening on {socket_path} and Muster has no daemon to start: no \
             binary was found beside the app and MUSTER_HERDR names none. This window will \
             render nothing. A bundle carries one (`./dev --bundle`), and an ordinary build \
             stages one beside the binary."
        ));
    };
    start(binary, socket_path, environment, locale, config_path, commands)?;
    Ok(Reached::Started)
}

/// Whether this daemon is one Muster just started or one it found already running.
///
/// The difference matters for exactly one thing, and it is not cosmetic: a daemon reads its
/// config when it starts. One Muster started is running the settings in the file; one it
/// adopted is running whatever it was started with, however long ago, and has to be asked to
/// read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reached {
    Started,
    Adopted,
}

/// Asks a daemon to read its config file again.
///
/// The lever that makes saving Muster's config file mean something. herdr reads its config at
/// startup and on request and watches no file, and Muster's daemon is started and never
/// stopped - so without this a changed setting would wait for a machine to be rebooted.
///
/// **What it cannot do is reach a pane that already exists.** herdr takes the shell and the
/// scrollback limit as arguments when it builds a pane's terminal, so both reach panes opened
/// afterwards and no others. The update checks are the exception and are the reason to call
/// this on a daemon Muster adopted: those are cancelled the moment the config is applied.
///
/// A failure here is Muster's own file being refused, so it is reported rather than returned:
/// there is nothing the caller can do about it and nothing about the window is wrong yet.
pub fn reload_configuration(socket_path: &str) {
    let answer = HerdrClient::new(socket_path).request("server.reload_config", &json!({}));
    match answer {
        Ok(result) => {
            let status = result
                .get("config_reload")
                .and_then(|reload| reload.get("status"))
                .and_then(|status| status.as_str())
                .unwrap_or("unknown");
            log::info(
                "daemon.config.reloaded",
                fields! {
                    "socket" => socket_path,
                    "status" => status,
                    "impact" => "panes opened from now on run these settings; panes already \
                                 open keep the ones they were made with, because the daemon \
                                 takes both when it builds a pane",
                },
            );
        }
        Err(failure) => {
            log::warn(
                "daemon.config.refused",
                fields! {
                    "socket" => socket_path,
                    "detail" => failure.to_string(),
                    "impact" => "the daemon is running the settings it was started with, so a \
                                 setting saved since then reaches no new pane either",
                    "check" => "the file named in daemon.starting - Muster wrote it, so a \
                                daemon refusing it is a bug here rather than in anybody's \
                                config",
                },
            );
        }
    }
}

/// The environment Muster resolves its own socket path from.
///
/// Read here rather than deeper down, so that everything below takes a map and is answerable
/// to the corpus without an environment (`crate::discovery`).
pub fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// What Muster's daemon is entitled to inherit from whoever launched Muster.
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
///
/// The daemon and its panes get one answer rather than two, because there is one environment:
/// a pane's program is a child of the daemon. That is worth stating rather than leaving
/// implicit - a future herdr with per-pane environments would let these come apart, and then
/// they are two decisions rather than one.
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
/// `LANGUAGE` beside it, which is Ghostty's pair and nothing else's. That is a loan, of the
/// same kind as the fonts and colours Muster used to take from a Ghostty config file: it is
/// invisible from here, it depends on the renderer being built before the daemon is started,
/// and it is the day a renderer changes that every pane silently drops to the C locale.
///
/// So Muster answers the question itself. `locale` is what the platform said, which only the
/// shell can ask. Whether a daemon gets it is decided here, and only when the environment
/// names *nothing* in the locale family: a `LANG` supplied beside an inherited `LC_CTYPE` is
/// the split locale [`is_carried`] already refuses to create, arrived at from the other
/// direction.
///
/// The other entry is `HERDR_CONFIG_PATH`, and it is here rather than beside `HERDR_SESSION`
/// on the command for one reason: it belongs in the answer to "what was this daemon given
/// that nobody gave Muster", which is the log line somebody reads when a pane runs the wrong
/// shell. It names a file Muster wrote from its own config, so that a `default_shell` set for
/// somebody's own terminal stops deciding what every Muster pane runs. Unlike a private
/// `XDG_CONFIG_HOME` it moves the config file and nothing else - the socket, the session state
/// and the data directory all stay where herdr's own rules put them, verified against the
/// pinned binary rather than only its source.
pub fn supplied(
    environment: &BTreeMap<String, String>,
    locale: Option<&str>,
    config_path: Option<&str>,
    commands: Option<&str>,
) -> BTreeMap<String, String> {
    let mut supplied = BTreeMap::new();
    if let Some(locale) = locale.filter(|locale| !locale.is_empty())
        && !names_a_locale(environment)
    {
        supplied.insert("LANG".to_string(), locale.to_string());
    }
    if let Some(path) = config_path.filter(|path| !path.is_empty()) {
        supplied.insert("HERDR_CONFIG_PATH".to_string(), path.to_string());
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

/// The variables Muster's daemon carries, and why each one is here.
///
/// Anything not on this list is a variable a pane's own shell can rebuild, or one that
/// belonged to whoever launched Muster and not to the agents Muster runs.
const CARRIED: &[&str] = &[
    // Where herdr's own config, sockets and session state live. These are also what Muster
    // resolved the socket path from, so a daemon started without them would bind somewhere
    // else and the launch would wait out its timeout for a daemon running perfectly well.
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
    // No pane has ever seen the daemon's: herdr sets `TERM=xterm-256color` per pane
    // unconditionally, because a pane is rendered by herdr's own terminal layer rather than by
    // whatever launched the app. The one thing that does read the daemon's own is herdr's
    // host-terminal detection, which decides who a notification is attributed to - so carrying
    // it meant a Muster launched from Ghostty had its daemon posting notifications as Ghostty,
    // to a terminal that is not there. A Dock launch never had one, so dropping it also makes
    // the two ways of starting Muster give the daemon the same environment.
    // The user's own ssh agent. A deliberate inclusion rather than an oversight: this is a
    // credential channel, and a pane that cannot `git push` or reach a devenv is a pane
    // somebody stops using. It is the person's own agent, it is what every terminal emulator
    // on this platform passes through, and unlike a harness's session token it belongs to the
    // human rather than to whichever program happened to launch Muster.
    "SSH_AUTH_SOCK",
];
