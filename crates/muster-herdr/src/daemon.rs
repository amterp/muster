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
use crate::discovery::OWN_SESSION;

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
/// The environment is inherited rather than built, because the socket path was computed from
/// it: a daemon started under a different `XDG_CONFIG_HOME` than the one Muster resolved
/// would bind somewhere else and this would wait out the timeout for a daemon that is running
/// perfectly well.
///
/// `HERDR_SESSION` rather than `--session`, so it reaches the panes the daemon spawns too.
/// That is what makes `herdr pane list` inside a Muster pane talk to the daemon that owns it
/// rather than to whatever the user's own herdr would find.
pub fn start(binary: &str, socket_path: &str) -> Result<(), String> {
    log::info(
        "daemon.starting",
        fields! {
            "binary" => binary,
            "socket" => socket_path,
            "session" => OWN_SESSION,
        },
    );

    let mut child = Command::new(binary)
        .arg("server")
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
pub fn ensure_running(socket_path: &str, binary: Option<&str>) -> Result<(), String> {
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
    start(binary, socket_path)
}

/// The environment Muster resolves its own socket path from.
///
/// Read here rather than deeper down, so that everything below takes a map and is answerable
/// to the corpus without an environment (`crate::discovery`).
pub fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}
