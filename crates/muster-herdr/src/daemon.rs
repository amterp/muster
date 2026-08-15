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
/// `HERDR_SESSION` rather than `--session`, so it reaches the panes the daemon spawns too.
/// That is what makes `herdr pane list` inside a Muster pane talk to the daemon that owns it
/// rather than to whatever the user's own herdr would find.
pub fn start(
    binary: &str,
    socket_path: &str,
    environment: &BTreeMap<String, String>,
) -> Result<(), String> {
    log::info(
        "daemon.starting",
        fields! {
            "binary" => binary,
            "socket" => socket_path,
            "session" => OWN_SESSION,
            // Whose config decides what a pane runs. Muster's daemon reads the user's own
            // herdr config, which it cannot be isolated from without moving every pane's
            // XDG_CONFIG_HOME too - so the file is named here, where somebody debugging a
            // pane that opened the wrong shell will find it.
            "config" => config_file(environment).unwrap_or_default(),
        },
    );

    let carried = carried(environment);
    let dropped: Vec<&str> = environment
        .keys()
        .filter(|name| !carried.contains_key(*name))
        .map(String::as_str)
        .collect();
    // Names, never values: this log is meant to be attachable to a bug report, and an
    // environment holds tokens (`architecture.md`, the diagnostic log). The names are what
    // somebody asking "why does my pane not see FOO" needs, and they are not secrets.
    log::info(
        "daemon.environment",
        fields! {
            "carried" => carried.keys().cloned().collect::<Vec<String>>().join(","),
            "dropped_count" => dropped.len().to_string(),
            "dropped" => dropped.join(","),
        },
    );

    let mut child = Command::new(binary)
        .arg("server")
        .env_clear()
        .envs(&carried)
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
) -> Result<(), String> {
    if answers(socket_path) {
        return Ok(());
    }
    let Some(binary) = binary else {
        return Err(format!(
            "nothing is listening on {socket_path} and Muster has no daemon to start: no \
             binary was found beside the app and MUSTER_HERDR names none. This window will \
             render nothing. A bundle carries one (`./dev --bundle`), and an ordinary build \
             stages one beside the binary."
        ));
    };
    start(binary, socket_path, environment)
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
    // scratch files somewhere unexpected.
    "LANG",
    "TZ",
    "TMPDIR",
    // What the terminal is. herdr sets this for a pane, but a daemon with none of its own has
    // nothing to fall back on when it starts a process outside one.
    "TERM",
    // The user's own ssh agent. A deliberate inclusion rather than an oversight: this is a
    // credential channel, and a pane that cannot `git push` or reach a devenv is a pane
    // somebody stops using. It is the person's own agent, it is what every terminal emulator
    // on this platform passes through, and unlike a harness's session token it belongs to the
    // human rather than to whichever program happened to launch Muster.
    "SSH_AUTH_SOCK",
];
