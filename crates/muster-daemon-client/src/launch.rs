//! Starting this machine's daemon, or adopting the one already running.
//!
//! A daemon is started and never stopped by a window quitting: sessions outlive the app, so the
//! app adopts whatever daemon answers on its socket and starts one only when none does
//! (MIP-3, section 1). The daemon's lock file settles two starters racing, and the loser exits
//! at once, so a caller that finds its own daemon gone simply dials the winner.
//!
//! A bundle's daemon is started through Launch Services, and any other is spawned ([`Route`]).
//! Either way the daemon repeats a token of this start's in its welcome, which is how a start
//! tells its own daemon from a rival's without a pid to compare.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::HandshakeError;
use muster_daemon_proto::{ConnectionKind, Welcome, install};

/// How long a daemon just started may take to answer.
///
/// It answers in milliseconds once it runs. The wait is for macOS, which holds a binary it has
/// not run before while it scans it: 16 s for a debug daemon on the machine this was measured
/// on, and 44 s for a freshly built app. That is every first launch after an install or an
/// update, not only the first on a machine, so every start gets it: a start keyed on "no
/// record yet" would still give up after an update, whose daemon is new to macOS but not to
/// Muster.
const START_PATIENCE: Duration = Duration::from_mins(1);

/// How long a start goes quietly before the log says it is slow, which is where the patience
/// stood before a first launch was known to need more.
const SLOW_START: Duration = Duration::from_secs(10);

/// How long a daemon that found the socket's lock held waits for the holder to answer before
/// trying again. A rival's daemon answers in milliseconds; one that does not was exiting.
const RIVAL_PATIENCE: Duration = Duration::from_millis(250);

/// How often a starting daemon is dialled.
const DIAL_INTERVAL: Duration = Duration::from_millis(2);

/// The daemon's exit status when another already serves its socket (`muster-daemon`'s
/// `ALREADY_SERVING`).
const ALREADY_SERVING: i32 = 3;

/// How long a daemon started through Launch Services may say something and still not answer
/// before what it said is taken as why it gave up.
const SAID_AND_SILENT: Duration = Duration::from_millis(250);

const OPEN: &str = "/usr/bin/open";

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
    let route = route(launch.binary);
    log::info(
        "daemon.starting",
        fields! {
            "binary" => launch.binary.display(),
            "socket" => launch.socket.display(),
            "stderr" => errors.display(),
            "route" => route.name(),
            "environment" => launch.environment.keys().cloned().collect::<Vec<_>>().join(","),
        },
    );
    let mut attempt = Attempt::begin(launch, &route, &errors)?;

    let began = Instant::now();
    let deadline = began + START_PATIENCE;
    let mut said_slow = false;
    // When this start's daemon found the socket's lock held, and nothing answered yet.
    let mut lost_the_race: Option<Instant> = None;
    loop {
        if let Ok(welcome) = probe(launch.socket) {
            // A rival starter's daemon can answer before this one's exit says it lost, and only
            // the daemon this start launched repeats its token.
            let reached =
                if welcome.launch == attempt.marker { Reached::Started } else { Reached::Adopted };
            log::info(
                "daemon.started",
                fields! {
                    "socket" => launch.socket.display(),
                    "daemon_pid" => welcome.pid,
                    "instance" => welcome.instance,
                    "reached" => format!("{reached:?}"),
                },
            );
            attempt.let_go();
            return Ok((reached, welcome));
        }
        if let Some(since) = lost_the_race
            && since.elapsed() >= RIVAL_PATIENCE
        {
            // Nothing came up behind the lock, so its holder was on its way out - a daemon just
            // asked to stop still holds it while it saves and closes. Try again now it is free.
            attempt.let_go();
            attempt = Attempt::begin(launch, &route, &errors)?;
            lost_the_race = None;
        }
        if lost_the_race.is_none() {
            match attempt.ended(&errors) {
                Some(Ended::LostTheRace) => lost_the_race = Some(Instant::now()),
                Some(Ended::Failed(how)) => {
                    return Err(format!(
                        "the daemon {how} before it answered on {}, so this window has no session \
                         behind it. It said: {}",
                        launch.socket.display(),
                        said(&errors, &attempt.marker)
                    ));
                }
                None => {}
            }
        }
        if !said_slow && began.elapsed() >= SLOW_START {
            said_slow = true;
            log::warn(
                "daemon.start.slow",
                fields! {
                    "socket" => launch.socket.display(),
                    "waited_s" => SLOW_START.as_secs(),
                    "patience_s" => START_PATIENCE.as_secs(),
                    "impact" => "this window has no panes until the daemon answers",
                    "check" => "macOS holds a binary it has not run before while it scans it, \
                                which is the usual cause on a first launch after an install or \
                                update; on any other launch, whether the machine is overloaded",
                },
            );
        }
        if Instant::now() >= deadline {
            let said = said(&errors, &attempt.marker);
            // It may yet answer, and must not be left to linger as a zombie when it ends.
            attempt.let_go();
            return Err(format!(
                "the daemon was started but did not answer on {} within {}s, so this window \
                 has no session behind it. It may still be starting, and relaunching will find \
                 it. It said: {said}",
                launch.socket.display(),
                START_PATIENCE.as_secs(),
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
}

/// How a daemon is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// As a child in a session of its own: a build's daemon, or one named outright.
    Spawn,
    /// Through Launch Services, as the helper application it is inside: a bundle's daemon.
    ///
    /// So macOS charges each pane's permission prompts to that bundle for as long as the daemon
    /// lives, under its own name, rather than to the app that spawned it and then to nothing
    /// once the app quits (`docs/observations/macos-26.4.1.md`, section 8).
    Open { bundle: PathBuf },
}

impl Route {
    fn name(&self) -> &'static str {
        match self {
            Route::Spawn => "spawn",
            Route::Open { .. } => "launch_services",
        }
    }
}

/// How `binary` is started: opened when it is the executable of an application bundle on a
/// Mac, which is the only thing Launch Services starts, and spawned otherwise.
pub fn route(binary: &Path) -> Route {
    if !cfg!(target_os = "macos") {
        return Route::Spawn;
    }
    let bundle = binary
        .parent()
        .filter(|directory| directory.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|bundle| bundle.extension().is_some_and(|extension| extension == "app"));
    match bundle {
        Some(bundle) => {
            Route::Open { bundle: std::path::absolute(bundle).unwrap_or(bundle.into()) }
        }
        None => Route::Spawn,
    }
}

/// What `open` is given to start `bundle`'s daemon.
///
/// `-n`, since a running instance of the helper is a daemon on another socket, not this one.
/// Its environment goes as `--env` and nothing else: `open` hands the application its own
/// environment with these on top, so `open` itself runs with none (`start`). `--stderr` appends,
/// which is what lets two starters share the file under a marker each.
pub fn open_arguments(
    bundle: &Path,
    launch: &Launch,
    errors: &Path,
    marker: &str,
) -> Vec<std::ffi::OsString> {
    let mut arguments: Vec<std::ffi::OsString> =
        vec!["-n".into(), "-a".into(), bundle.into(), "--stderr".into(), errors.into()];
    for (name, value) in launch.environment {
        arguments.push("--env".into());
        arguments.push(format!("{name}={value}").into());
    }
    arguments.push("--args".into());
    arguments.extend(daemon_arguments(launch, marker));
    arguments
}

fn daemon_arguments(launch: &Launch, marker: &str) -> Vec<std::ffi::OsString> {
    let mut arguments: Vec<std::ffi::OsString> = vec!["--socket".into(), launch.socket.into()];
    if let Some(data) = launch.data {
        arguments.push("--data".into());
        arguments.push(data.into());
    }
    arguments.push("--launch".into());
    arguments.push(marker.into());
    arguments
}

/// One try at starting the daemon, under a marker of its own in the stderr file.
struct Attempt {
    marker: String,
    /// The daemon itself when spawned. Through Launch Services there is no child to hold:
    /// `open` has returned, and the daemon's parent is launchd.
    child: Option<Child>,
    /// When the daemon, opened, first said something while not answering.
    said_since: Option<Instant>,
}

/// How an attempt ended without an answer.
enum Ended {
    /// Another daemon holds the socket's lock: a rival starter's coming up, or one on its way
    /// out.
    LostTheRace,
    /// Anything else, said as the rest of a sentence: "exited with status 1".
    Failed(String),
}

impl Attempt {
    fn begin(launch: &Launch, route: &Route, errors: &Path) -> Result<Attempt, String> {
        let marker = crate::start_marker();
        mark(launch.socket, errors, &marker).map_err(|error| {
            format!(
                "could not write {} before starting the daemon ({error}), so this window has no \
                 session behind it. Check that the directory is writable.",
                errors.display()
            )
        })?;
        let child = match route {
            Route::Spawn => Some(spawn(launch, errors, &marker).map_err(|error| {
                format!(
                    "could not run the daemon at {} ({error}), so this window has no session \
                     behind it. A build stages it beside the bridge; check that it is there and \
                     executable.",
                    launch.binary.display()
                )
            })?),
            Route::Open { bundle } => {
                open(bundle, launch, errors, &marker)?;
                None
            }
        };
        Ok(Attempt { marker, child, said_since: None })
    }

    /// Whether the daemon has given up without answering, and why.
    fn ended(&mut self, errors: &Path) -> Option<Ended> {
        if let Some(child) = &mut self.child {
            let status = child.try_wait().ok()??;
            return Some(if status.code() == Some(ALREADY_SERVING) {
                Ended::LostTheRace
            } else {
                Ended::Failed(format!("exited with {status}"))
            });
        }
        // Opened, so its exit is launchd's to see: what it wrote is the only sign. A daemon
        // that answers writes nothing there, so words while it is silent are why - given a
        // moment, since a daemon can write a line and then answer.
        let text = std::fs::read_to_string(errors).unwrap_or_default();
        let said = crate::after_marker(&text, &self.marker);
        if said.is_empty() {
            return None;
        }
        if said.contains(install::ANOTHER_SERVES) {
            return Some(Ended::LostTheRace);
        }
        let since = *self.said_since.get_or_insert_with(Instant::now);
        (since.elapsed() >= SAID_AND_SILENT).then(|| Ended::Failed("gave up".to_string()))
    }

    /// Stops watching the daemon, leaving it running.
    fn let_go(self) {
        if let Some(child) = self.child {
            reap_later(child);
        }
    }
}

/// Appends this start's marker line to the stderr file, creating the directory beside the
/// socket first: the daemon makes it when it claims the socket, but the file is opened before
/// that, and on a machine that never ran a daemon it would not be there.
fn mark(socket: &Path, errors: &Path, marker: &str) -> std::io::Result<()> {
    if let Some(directory) = socket.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let mut errors = std::fs::OpenOptions::new().create(true).append(true).open(errors)?;
    writeln!(errors, "{marker}")
}

fn spawn(launch: &Launch, errors: &Path, marker: &str) -> std::io::Result<Child> {
    // Appended to, under a line of this start's own: two starters racing share the file, and
    // each quotes only what its own daemon said.
    let errors = std::fs::OpenOptions::new().append(true).open(errors)?;
    let mut command = Command::new(launch.binary);
    command
        .args(daemon_arguments(launch, marker))
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

/// Has Launch Services start the helper, and returns once it has: `open` waits for the
/// application to launch, not to answer.
fn open(bundle: &Path, launch: &Launch, errors: &Path, marker: &str) -> Result<(), String> {
    let output = Command::new(OPEN)
        .env_clear()
        .args(open_arguments(bundle, launch, errors, marker))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "could not run {OPEN} to start {} ({error}), so this window has no session \
                 behind it.",
                bundle.display()
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "Launch Services would not start {} ({}: {}), so this window has no session behind \
         it. A damaged or unsigned helper is the usual cause; `codesign --verify --deep` on \
         the app says which.",
        bundle.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
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
