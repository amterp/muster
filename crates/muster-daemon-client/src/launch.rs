//! Starting this machine's daemon, or adopting the one already running.
//!
//! A daemon is started and never stopped by a window quitting: sessions outlive the app, so the
//! app adopts whatever daemon answers on its socket and starts one only when none does
//! (MIP-3, section 1). The daemon's lock file settles two starters racing, and the loser exits
//! at once, so a caller that finds its own daemon gone simply dials the winner.
//!
//! Spawned directly, in a session of its own. On the Mac a daemon should be started through
//! Launch Services, so macOS charges each pane's permission prompts to the daemon's own bundle
//! rather than to the app (`docs/observations/macos-26.4.1.md`, section 8); until the helper
//! bundle holds muster-daemon, a spawned daemon's prompts are charged to Muster.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::HandshakeError;
use muster_daemon_proto::{ConnectionKind, Welcome};

/// How long a daemon just started may take to answer. It answers in milliseconds; this is for a
/// machine so loaded that starting any process is slow.
const START_PATIENCE: Duration = Duration::from_secs(10);

/// How often a starting daemon is dialled.
const DIAL_INTERVAL: Duration = Duration::from_millis(2);

/// The daemon's exit status when another already serves its socket (`muster-daemon`'s
/// `ALREADY_SERVING`).
const ALREADY_SERVING: i32 = 3;

/// What to start, if nothing answers.
#[derive(Debug)]
pub struct Launch<'a> {
    pub binary: &'a Path,
    /// The data directory its shells are given. Absent: the one beside the binary.
    pub data: Option<&'a Path>,
    pub socket: &'a Path,
    /// The daemon's whole environment, and so every pane's starting point. It outlives the
    /// app, so the caller builds it from an allowlist rather than passing its own.
    pub environment: &'a BTreeMap<String, String>,
}

/// Whether the daemon was started here or found running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reached {
    Started,
    Adopted,
}

/// Dials `launch.socket`, and starts the daemon there if nothing answers.
///
/// Returns who answered. A daemon that answers and refuses this client's protocol is an error,
/// never a reason to start a second one: it holds somebody's panes.
pub fn ensure_running(launch: &Launch) -> Result<(Reached, Welcome), String> {
    match probe(launch.socket) {
        Ok(welcome) => return Ok((Reached::Adopted, welcome)),
        Err(HandshakeError::Unreachable(_)) => {}
        Err(HandshakeError::Stalled(why)) => {
            return Err(format!(
                "a daemon holds {} and is not answering: {why}. Its panes are absent from this \
                 window, and starting another daemon would not help, since the socket is \
                 taken. Check whether it is stopped or stuck (`ps -o stat,pid,command` on the \
                 pid its log names), and look at its log beside the socket.",
                launch.socket.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "the daemon on {} would not talk to this app: {error}. Its panes are still \
                 running, and this window cannot show them until a Muster that speaks its \
                 protocol is used.",
                launch.socket.display()
            ));
        }
    }
    start(launch)
}

fn start(launch: &Launch) -> Result<(Reached, Welcome), String> {
    let errors = stderr_path(launch.socket);
    log::info(
        "daemon.starting",
        fields! {
            "binary" => launch.binary.display(),
            "socket" => launch.socket.display(),
            "stderr" => errors.display(),
            "environment" => launch.environment.keys().cloned().collect::<Vec<_>>().join(","),
        },
    );
    let marker = crate::start_marker();
    let mut child = spawn(launch, &errors, &marker).map_err(|error| {
        format!(
            "could not run the daemon at {} ({error}), so this window has no session behind it. \
             A build stages it beside the bridge; check that it is there and executable.",
            launch.binary.display()
        )
    })?;

    let deadline = Instant::now() + START_PATIENCE;
    let mut lost_the_race = false;
    loop {
        if let Ok(welcome) = probe(launch.socket) {
            // A rival starter's daemon can answer before this one's exit says it lost.
            let reached =
                if welcome.pid == child.id() { Reached::Started } else { Reached::Adopted };
            log::info(
                "daemon.started",
                fields! {
                    "socket" => launch.socket.display(),
                    "pid" => welcome.pid,
                    "instance" => welcome.instance,
                    "reached" => format!("{reached:?}"),
                },
            );
            reap_later(child);
            return Ok((reached, welcome));
        }
        if !lost_the_race && let Ok(Some(status)) = child.try_wait() {
            if status.code() == Some(ALREADY_SERVING) {
                // Another starter won; its daemon is coming up on this socket.
                lost_the_race = true;
            } else {
                return Err(format!(
                    "the daemon exited with {status} before it answered on {}, so this window \
                     has no session behind it. It said: {}",
                    launch.socket.display(),
                    said(&errors, &marker)
                ));
            }
        }
        if Instant::now() >= deadline {
            // It may yet answer, and must not be left to linger as a zombie when it ends.
            reap_later(child);
            return Err(format!(
                "the daemon was started but did not answer on {} within {}s, so this window \
                 has no session behind it. It may still be starting, and relaunching will find \
                 it. It said: {}",
                launch.socket.display(),
                START_PATIENCE.as_secs(),
                said(&errors, &marker)
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
}

fn spawn(launch: &Launch, errors: &Path, marker: &str) -> std::io::Result<Child> {
    // The daemon makes its own directory when it claims the socket, but the stderr file beside
    // the socket is opened first: on a machine that never ran a daemon it would not be there.
    if let Some(directory) = launch.socket.parent() {
        std::fs::create_dir_all(directory)?;
    }
    // Appended to, under a line of this start's own: two starters racing share the file, and
    // each quotes only what its own daemon said.
    let mut errors = std::fs::OpenOptions::new().create(true).append(true).open(errors)?;
    writeln!(errors, "{marker}")?;
    let mut command = Command::new(launch.binary);
    command.arg("--socket").arg(launch.socket);
    if let Some(data) = launch.data {
        command.arg("--data").arg(data);
    }
    command
        .env_clear()
        .envs(launch.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // What the daemon says before its own log is open: a socket path too long to bind, a
        // data directory that is incomplete.
        .stderr(errors);
    // SAFETY: setsid is async-signal-safe and touches nothing the parent shares.
    unsafe {
        // A session of its own, so neither the app quitting nor the terminal it was launched
        // from closing takes every agent with it.
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Waits for the daemon on a thread of its own, so that when it ends, however long from now, it
/// does not linger as a zombie of the app's.
fn reap_later(mut child: Child) {
    let _ =
        std::thread::Builder::new().name("muster-daemon-reaper".into()).spawn(move || child.wait());
}

/// Ends the daemon on `socket` and every pane in it, and waits up to `patience` for it to go.
pub fn stop(socket: &Path, patience: Duration) -> Result<(), String> {
    let control =
        crate::control::Control::open(socket, "muster stop", |_, _| {}).map_err(|error| {
            format!(
                "could not reach the daemon on {} to stop it ({error}), so nothing was stopped. \
                 If no daemon runs there, there is nothing to stop; if one does, its log beside \
                 the socket says why it would not talk.",
                socket.display()
            )
        })?;
    control.stop().wait(patience).map_err(|why| {
        format!(
            "the daemon on {} was asked to stop and {why}, so it may still be running with its \
             panes, or be part way through closing them. Dial it again to see whether it \
             answers; its log beside the socket says what it did with the request.",
            socket.display()
        )
    })?;
    let deadline = Instant::now() + patience;
    while probe(socket).is_ok() {
        if Instant::now() >= deadline {
            return Err(format!(
                "the daemon on {} agreed to stop and is still answering after {patience:?}; its \
                 panes may still be closing",
                socket.display()
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
    Ok(())
}

/// Whether a daemon answers on `socket`, by its handshake: a socket file outlives the daemon
/// that made it, so its presence says nothing.
fn probe(socket: &Path) -> Result<Welcome, HandshakeError> {
    crate::dial(socket, ConnectionKind::Control, "muster probe").map(|(_, welcome)| welcome)
}

/// Where a daemon started here writes what it says before its log is open: beside its socket,
/// where the daemon keeps its log and state.
fn stderr_path(socket: &Path) -> PathBuf {
    socket.with_extension("stderr")
}

fn said(errors: &Path, marker: &str) -> String {
    let text = std::fs::read_to_string(errors).unwrap_or_default();
    match crate::after_marker(&text, marker) {
        "" => format!("nothing (in {} after the line {marker})", errors.display()),
        said => said.to_string(),
    }
}
