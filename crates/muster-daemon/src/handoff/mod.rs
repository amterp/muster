//! Replacing a running daemon without ending a pane (MIP-3 section 10).
//!
//! The daemon being replaced starts its successor with one end of a socket pair and hands it,
//! in order, the socket, the session, and every pane: its PTY master, its record and a replay
//! of its terminal. Until the successor says it serves, the old daemon keeps its own copy of
//! every descriptor and has only paused - its accept loop, its persister, each pane's reader -
//! so a successor that fails at any step takes nothing with it, and the old daemon goes on.
//!
//! Connections are not carried over. Each one ends when the old daemon exits, and its client
//! connects again to the same socket, where the new daemon serves.

mod fds;

use std::os::fd::{AsFd, AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::version::{PROTOCOL, compatible};
use muster_daemon_proto::{self as proto, connection, handoff};

use crate::daemon_log::DaemonLog;
use crate::data::Data;
use crate::descriptors::Sealing;
use crate::effects::Reported;
use crate::persist::{self, Persister};
use crate::pty::Grid;
use crate::server::Socket;
use crate::session::{Handing, Places, Replacement, Reply, Saved, Shared, Stop};

/// How long either side waits for the other at one step before giving the handoff up.
const STEP: Duration = Duration::from_secs(10);

/// How long the successor has to answer `--version`, which it is run with once before any pane
/// is touched. macOS checks a binary the first time it runs, which took up to 12.6 s on a busy
/// machine: inside a step of the handoff that would outlast [`STEP`], and here it costs only
/// the wait.
const LAUNCH: Duration = Duration::from_secs(30);

/// A replay goes in pieces of this, each well inside the largest frame.
const PIECE: usize = 1 << 20;

/// The descriptor the successor finds its end of the socket pair on.
const LINK: RawFd = 3;

/// How long the old daemon waits for what it told its subscribers to be written before exiting.
const FLUSH: Duration = Duration::from_secs(1);

/// Test-only faults, a comma-separated list read by both daemons, each acting on its own: the
/// daemon taking over `refuse`, `exit-before-ready`, `exit-after-commit`, `pause-after-accept`,
/// `pause-before-ready` and `pause-before-serving`, and the daemon handing over
/// `pause-before-hold`, `pause-after-serving`, `report-before-settle`, which queues a report just before it waits
/// for the reports, as a reader does that reports as it parks, and `short-launch`, which gives
/// the successor one second rather than [`LAUNCH`] to answer `--version`.
const FAULT: &str = "MUSTER_DAEMON_HANDOFF_FAULT";

/// The faults this daemon was started with. Read only by a debug build, which is what every test
/// runs: a shipped daemon never reads the variable, so nothing in a user's environment can make
/// one fail a handoff.
#[derive(Debug, Default)]
struct Faults(Vec<String>);

impl Faults {
    fn read() -> Faults {
        if cfg!(debug_assertions) {
            let faults = std::env::var(FAULT).unwrap_or_default();
            Faults(faults.split(',').filter(|f| !f.is_empty()).map(str::to_string).collect())
        } else {
            Faults::default()
        }
    }

    fn has(&self, fault: &str) -> bool {
        self.0.iter().any(|named| named == fault)
    }

    /// With `pause-<step>`, writes this daemon's pid to `<socket>.handoff-paused` and waits
    /// until a test removes it, so the test can land a signal or a kill at exactly this step.
    fn pause(&self, step: &str, socket: &Path) {
        if !self.has(&format!("pause-{step}")) {
            return;
        }
        let mut marker = socket.as_os_str().to_owned();
        marker.push(".handoff-paused");
        let marker = std::path::PathBuf::from(marker);
        if std::fs::write(&marker, std::process::id().to_string()).is_err() {
            return;
        }
        let deadline = Instant::now() + Duration::from_mins(1);
        while marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn send(link: &mut UnixStream, message: handoff::Message) -> Result<(), String> {
    connection::send(link, &proto::Handoff { message: Some(message) })
        .map_err(|error| format!("could not send to the other daemon: {error}"))
}

fn receive(link: &mut UnixStream, awaited: &str) -> Result<handoff::Message, String> {
    match connection::receive::<proto::Handoff>(link) {
        Ok(Some(proto::Handoff { message: Some(message) })) => Ok(message),
        Ok(Some(_)) => Err(format!("an empty message came instead of {awaited}")),
        Ok(None) => Err(format!("the other daemon hung up before {awaited}")),
        Err(error) => Err(format!("nothing readable came as {awaited}: {error}")),
    }
}

fn unexpected(awaited: &str, came: &handoff::Message) -> String {
    let came = match came {
        handoff::Message::Refused(refused) => return format!("it refused: {}", refused.reason),
        handoff::Message::Offer(_) => "an offer",
        handoff::Message::Accept(_) => "an accept",
        handoff::Message::Session(_) => "the session",
        handoff::Message::Pane(_) => "a pane",
        handoff::Message::ReplayPiece(_) => "a piece of replay",
        handoff::Message::Ready(_) => "ready",
        handoff::Message::Commit(_) => "commit",
        handoff::Message::Serving(_) => "serving",
    };
    format!("{came} came where {awaited} belonged")
}

// ---------------------------------------------------------------------------------------------
// Handing over

/// Hands every pane to the daemon `replacement` names. Done means it serves the socket, and
/// this daemon is to exit; anything short of that is undone, and refused with the reason.
///
/// The program is run once first, with the session unlocked and taking changes: the first run
/// of a new binary can take seconds on macOS, and nothing is refused until the panes are being
/// handed over.
pub(crate) fn hand_over(shared: &Arc<Shared>, replacement: &Replacement) -> Reply {
    let started = Instant::now();
    let faults = Faults::read();
    let patience = if faults.has("short-launch") { Duration::from_secs(1) } else { LAUNCH };
    if let Err(why) = launch(&replacement.program, patience) {
        return failed(replacement, &why);
    }
    if let Err(why) = shared.lock().begin_replacing() {
        return failed(replacement, why);
    }
    let mut handing = shared.lock().handing();
    log::info(
        "daemon.handoff.started",
        fields! {
            "program" => replacement.program.display(),
            "panes" => handing.panes.len(),
        },
    );
    let mut successor = None;
    match handed(shared, replacement, &mut handing, &mut successor) {
        Ok(accepted) => {
            let pid = successor.as_ref().map_or(0, Child::id);
            let subscribers = shared.lock().replaced(pid, accepted.daemon_version.clone());
            faults.pause("after-serving", &shared.socket.path);
            for pane in &handing.panes {
                pane.io.close(proto::DetachReason::Replaced);
                let dropped = pane.io.dropped_at_handoff();
                if !dropped.is_empty() {
                    log::warn(
                        "daemon.handoff.perform_dropped",
                        fields! {
                            "pane" => pane.record.pane,
                            "dropped" => format!("{dropped:?}"),
                            "impact" => "a reset or clear_screen asked of the pane while it was \
                                         handed over was not done; its surface and terminal \
                                         agree, and still show what was there",
                            "check" => "nothing is wrong; asking again now does it",
                        },
                    );
                }
            }
            let deadline = Instant::now() + FLUSH;
            for subscriber in subscribers {
                subscriber.flush(deadline.saturating_duration_since(Instant::now()));
            }
            log::info(
                "daemon.handoff.done",
                fields! {
                    "successor_pid" => pid,
                    "version" => accepted.daemon_version,
                    "ms" => started.elapsed().as_millis(),
                },
            );
            Reply::done()
        }
        Err(why) => {
            // The successor first, so no pane is ever read by two daemons at once.
            if let Some(mut successor) = successor {
                let _ = successor.kill();
                let _ = successor.wait();
            }
            for pane in &handing.panes {
                pane.io.release_reader();
            }
            shared.socket.accepting.release();
            // A new daemon that died after committing had pointed it at itself.
            shared.point_link();
            handing.persister.resume();
            if let Some(log) = &handing.log {
                log.write_file();
                let records = log.take_in_handed_over();
                if records > 0 {
                    log::info("daemon.handoff.successor_log", fields! { "records" => records });
                }
            }
            let stop_deferred = shared.lock().not_replaced();
            let reply = failed(replacement, &why);
            if stop_deferred {
                let _ = shared.stopping.send(Stop::Asked);
            }
            reply
        }
    }
}

fn failed(replacement: &Replacement, why: &str) -> Reply {
    log::error(
        "daemon.handoff.failed",
        fields! {
            "program" => replacement.program.display(),
            "error" => why,
            "impact" => "no pane was handed over; this daemon goes on serving every pane as \
                         before",
            "check" => "the new daemon's own error, on this daemon's stderr, and whether the \
                        program is a muster-daemon of the same protocol major",
        },
    );
    Reply::refused(format!("could not hand over to {}: {why}", replacement.program.display()))
}

fn handed(
    shared: &Shared,
    replacement: &Replacement,
    handing: &mut Handing,
    successor: &mut Option<Child>,
) -> Result<handoff::Accept, String> {
    // Two daemons writing the state file at once would share its temporary file.
    if !handing.persister.pause(STEP) {
        return Err("this daemon's write of its state file did not finish".to_string());
    }
    if !shared.socket.accepting.hold(STEP) {
        return Err("this daemon's accept loop did not stop".to_string());
    }
    if let Some(log) = &handing.log {
        log.forget_handed_over();
    }
    let (mut link, child) = start(replacement, &shared.socket.path)?;
    *successor = Some(child);
    let offer = handoff::Offer {
        protocol: Some(PROTOCOL),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        panes: u32::try_from(handing.panes.len()).map_err(|error| error.to_string())?,
    };
    send(&mut link, handoff::Message::Offer(offer))?;
    let accepted = match receive(&mut link, "an accept")? {
        handoff::Message::Accept(accept) => accept,
        other => return Err(unexpected("an accept", &other)),
    };

    Faults::read().pause("before-hold", &shared.socket.path);
    // Every reader held first, and what they reported applied, before anything is captured:
    // a title or directory that changed after the capture would reach the replay but not the
    // record.
    for pane in &handing.panes {
        if !pane.io.hold_reader(STEP) {
            return Err(format!("pane {}'s reader did not stop", pane.record.pane));
        }
    }
    let reports = shared.lock().reports();
    if Faults::read().has("report-before-settle") {
        reports.send(0, Reported::Title(String::new()));
    }
    if !reports.settle(STEP) {
        return Err("what the panes reported was not applied in time".to_string());
    }
    shared.lock().recapture(handing)?;

    let state = serde_json::to_vec(&handing.state).map_err(|error| error.to_string())?;
    let socket = &shared.socket;
    fds::send(&link, &[socket.listener.as_fd(), socket.lock.as_fd()])
        .map_err(|error| format!("could not send the socket: {error}"))?;
    let app_manifests = handing
        .app_manifests
        .iter()
        .map(|(agent, toml)| proto::Manifest { agent: agent.clone(), toml: toml.clone() })
        .collect();
    let saving_stopped = !handing.persister.saving();
    send(
        &mut link,
        handoff::Message::Session(handoff::Session { state, app_manifests, saving_stopped }),
    )?;

    for pane in &handing.panes {
        let name = &pane.record.pane;
        let (replay, grid) = pane.io.replay();
        fds::send(&link, &[pane.io.master()])
            .map_err(|error| format!("could not send pane {name}'s terminal: {error}"))?;
        let grid = proto::Grid {
            cols: grid.cols.into(),
            rows: grid.rows.into(),
            width_px: grid.width_px.into(),
            height_px: grid.height_px.into(),
        };
        send(
            &mut link,
            handoff::Message::Pane(Box::new(handoff::Pane {
                record: Some(pane.record.clone()),
                grid: Some(grid),
                process: pane.process,
                replay_length: replay.len() as u64,
                detection: pane.io.carried_detection(),
            })),
        )?;
        for piece in replay.chunks(PIECE) {
            send(&mut link, handoff::Message::ReplayPiece(piece.to_vec()))?;
        }
    }

    match receive(&mut link, "ready")? {
        handoff::Message::Ready(_) => {}
        other => return Err(unexpected("ready", &other)),
    }
    log::info(
        "daemon.handoff.committed",
        fields! { "successor_pid" => successor.as_ref().map_or(0, Child::id) },
    );
    // From here the new daemon writes the log's file; this one keeps its records to itself.
    if let Some(log) = &handing.log {
        log.withhold_file();
    }
    send(&mut link, handoff::Message::Commit(handoff::Commit {}))?;
    match receive(&mut link, "serving")? {
        handoff::Message::Serving(_) => Ok(accepted),
        other => Err(unexpected("serving", &other)),
    }
}

/// Runs the successor once with `--version`, before anything is paused, so a program that
/// cannot start - a bad build, a missing library, or macOS's first check of a new binary
/// outlasting a step - is refused here rather than partway through.
fn launch(program: &Path, patience: Duration) -> Result<(), String> {
    let started = Instant::now();
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start it: {error}"))?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < patience => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not answer --version within {} s; nothing was handed over. A new \
                     binary is checked by macOS the first time it runs, which can take this long \
                     on a busy machine, so asking again may succeed",
                    patience.as_secs()
                ));
            }
            Err(error) => return Err(format!("could not wait for its --version: {error}")),
        }
    };
    if !status.success() {
        return Err(format!("it answered --version with {status}; nothing was handed over"));
    }
    log::info(
        "daemon.handoff.launched",
        fields! {
            "program" => program.display(),
            "ms" => started.elapsed().as_millis(),
        },
    );
    Ok(())
}

/// Starts the successor, on its own session and with no signal blocked, with one end of a
/// socket pair as descriptor [`LINK`] and no other descriptor of this daemon's.
fn start(replacement: &Replacement, socket: &Path) -> Result<(UnixStream, Child), String> {
    let (link, theirs) = UnixStream::pair().map_err(|error| error.to_string())?;
    let mut command = Command::new(&replacement.program);
    command.arg("--socket").arg(socket).arg("--handoff").arg(LINK.to_string());
    if let Some(data) = &replacement.data {
        command.arg("--data").arg(data);
    }
    command.stdin(Stdio::null());
    let passed = theirs.as_raw_fd();
    let mut sealing = Sealing::prepare();
    // SAFETY: the closure runs in the child between fork and exec, and makes only
    // async-signal-safe calls: what `seal` calls, dup2, fcntl, sigemptyset, sigprocmask and
    // setsid. `passed` stays open in the parent until the spawn returns.
    unsafe {
        command.pre_exec(move || {
            sealing.seal();
            // dup2 leaves the copy inheritable, but not when the two numbers are the same.
            if libc::dup2(passed, LINK) == -1 || libc::fcntl(LINK, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let mut unblocked = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(unblocked.as_mut_ptr());
            if libc::sigprocmask(libc::SIG_SETMASK, unblocked.as_ptr(), std::ptr::null_mut()) == -1
                || libc::setsid() == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|error| format!("could not start it: {error}"))?;
    drop(theirs);
    for set in [link.set_read_timeout(Some(STEP)), link.set_write_timeout(Some(STEP))] {
        set.map_err(|error| error.to_string())?;
    }
    Ok((link, child))
}

// ---------------------------------------------------------------------------------------------
// Taking over

/// A daemon that has taken over and serves: what `main` waits on as it would for any daemon.
pub(crate) struct TakenOver {
    pub(crate) shared: Arc<Shared>,
    pub(crate) stop: Receiver<Stop>,
    pub(crate) persister: Arc<Persister>,
}

/// Takes the socket, the session and every pane from the daemon handing them over on `link`,
/// and serves once it commits. Any error before then leaves every pane with that daemon.
pub(crate) fn take_over(
    link: RawFd,
    socket: &Path,
    data: Option<&Path>,
    signals: libc::sigset_t,
) -> Result<TakenOver, String> {
    // SAFETY: the daemon that started this one passed its end of a socket pair as `link`, and
    // nothing else here uses that descriptor.
    let mut link = unsafe { UnixStream::from_raw_fd(link) };
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe { libc::fcntl(link.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    // Each pane's reader is held before its pane is sent, which takes up to a step.
    link.set_read_timeout(Some(STEP * 3)).map_err(|error| error.to_string())?;
    let faults = Faults::read();
    let log = DaemonLog::start(socket, true);

    let offer = match receive(&mut link, "the offer")? {
        handoff::Message::Offer(offer) => offer,
        other => return Err(unexpected("the offer", &other)),
    };
    log::info(
        "daemon.handoff.taking_over",
        fields! {
            "from_pid" => offer.pid,
            "from_version" => offer.daemon_version,
            "panes" => offer.panes,
        },
    );
    let theirs = offer.protocol.unwrap_or_default();
    if !compatible(&PROTOCOL, &theirs) {
        return refuse(
            &mut link,
            format!("this daemon speaks protocol {PROTOCOL}, and the one handing over {theirs}"),
        );
    }
    if faults.has("refuse") {
        return refuse(&mut link, format!("{FAULT}=refuse"));
    }
    let accept = handoff::Accept {
        protocol: Some(PROTOCOL),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    send(&mut link, handoff::Message::Accept(accept))?;
    faults.pause("after-accept", socket);

    let Built { shared, stopping, stop, persister, tabs, app_manifests } =
        build(&mut link, socket, data, log)?;
    if !app_manifests.is_empty() {
        shared.adopt_manifests(app_manifests);
    }
    adopt_panes(&mut link, &shared, offer.panes)?;
    let tabs = shared.lock().adopt_tabs(tabs);
    if let Err(problem) = tabs {
        return refuse(&mut link, problem);
    }
    faults.pause("before-ready", socket);
    if faults.has("exit-before-ready") {
        std::process::exit(1);
    }
    send(&mut link, handoff::Message::Ready(handoff::Ready {}))?;

    match receive(&mut link, "commit") {
        Ok(handoff::Message::Commit(_)) => {}
        Ok(other) => return Err(unexpected("commit", &other)),
        Err(why) => {
            return Err(format!("{why}; every pane stays with the daemon handing over"));
        }
    }
    if let Some(log) = shared.lock().log() {
        log.write_file();
    }
    shared.lock().release_readers();
    shared.point_link();
    crate::serve(&shared, signals, stopping)?;
    persister.arm();
    persister.changed();
    faults.pause("before-serving", socket);
    if faults.has("exit-after-commit") {
        std::process::exit(1);
    }
    // This daemon holds every pane from the commit on, so a daemon handing over that has gone
    // since is no reason to stop: exiting now would hang up every pane with nobody to take them.
    if let Err(why) = send(&mut link, handoff::Message::Serving(handoff::Serving {})) {
        log::warn(
            "daemon.handoff.unconfirmed",
            fields! {
                "from_pid" => offer.pid,
                "error" => why,
                "impact" => "none for the panes: this daemon serves every one of them; the \
                             request that asked for the handoff was not answered",
                "check" => "how the daemon handing over ended, on its stderr, since it went away \
                            between committing and exiting on its own",
            },
        );
    }
    log::info(
        "daemon.handoff.serving",
        fields! {
            "socket" => shared.socket.path.display(),
            "version" => env!("CARGO_PKG_VERSION"),
            "from_pid" => offer.pid,
            "from_version" => offer.daemon_version,
        },
    );
    Ok(TakenOver { shared, stop, persister })
}

/// The daemon this one becomes, built from the socket and the session handed over, with no pane
/// yet and nothing served.
struct Built {
    shared: Arc<Shared>,
    stopping: Sender<Stop>,
    stop: Receiver<Stop>,
    persister: Arc<Persister>,
    tabs: Vec<persist::Tab>,
    app_manifests: Vec<proto::Manifest>,
}

fn build(
    link: &mut UnixStream,
    socket: &Path,
    data: Option<&Path>,
    log: Option<Arc<DaemonLog>>,
) -> Result<Built, String> {
    let mut handed = fds::receive(link, 2)
        .map_err(|error| format!("the socket did not arrive: {error}"))?
        .into_iter();
    let (Some(listener), Some(lock)) = (handed.next(), handed.next()) else {
        unreachable!("receive checks the count")
    };
    let session = match receive(link, "the session")? {
        handoff::Message::Session(session) => session,
        other => return Err(unexpected("the session", &other)),
    };
    let state = match persist::parse(&session.state) {
        persist::Loaded::State(state) => state,
        persist::Loaded::Newer(version) => {
            return refuse(
                link,
                format!("the session is in format {version}, newer than this daemon reads"),
            );
        }
        other => return refuse(link, format!("the session is not readable: {other:?}")),
    };
    let data = match Data::locate(data) {
        Ok(data) => data,
        Err(problem) => return refuse(link, problem),
    };
    let socket = match Socket::new(socket.to_path_buf(), listener.into(), lock.into()) {
        Ok(socket) => socket,
        Err(error) => return refuse(link, format!("could not take the socket: {error}")),
    };
    let persister = Persister::new(persist::path_for(&socket.path), session.saving_stopped);
    if session.saving_stopped {
        log::warn(
            "daemon.state.not_saving",
            fields! {
                "path" => persister.path().display(),
                "impact" => "the daemon handing over saved nothing over this file, and neither \
                             does this one; a restart brings back what the file held",
                "check" => "the handing daemon's log for why it stopped saving",
            },
        );
    }
    let saved = Saved {
        persister: Arc::clone(&persister),
        settings: Some(state.settings.clone()),
        restoring: false,
    };
    let places = Places::of_this_process(&socket.path, data, log);
    let (stopping, stop) = mpsc::channel();
    let inherited: Vec<_> = std::env::vars_os().collect();
    let shared = Shared::new(crate::instance(), stopping.clone(), inherited, places, saved, socket);

    Ok(Built {
        shared,
        stopping,
        stop,
        persister,
        tabs: state.tabs,
        app_manifests: session.app_manifests,
    })
}

/// Takes each pane as it arrives, and refuses the handoff at one this daemon cannot take.
fn adopt_panes(link: &mut UnixStream, shared: &Shared, panes: u32) -> Result<(), String> {
    for _ in 0..panes {
        let master = fds::receive(link, 1)
            .map_err(|error| format!("a pane's terminal did not arrive: {error}"))?
            .remove(0);
        let pane = match receive(link, "a pane")? {
            handoff::Message::Pane(pane) => pane,
            other => return Err(unexpected("a pane", &other)),
        };
        let mut replay = Vec::with_capacity(usize::try_from(pane.replay_length).unwrap_or(0));
        while (replay.len() as u64) < pane.replay_length {
            match receive(link, "a piece of replay")? {
                handoff::Message::ReplayPiece(piece) => replay.extend_from_slice(&piece),
                other => return Err(unexpected("a piece of replay", &other)),
            }
        }
        let grid = pane.grid.unwrap_or_default();
        let grid = Grid {
            cols: u16::try_from(grid.cols).unwrap_or(u16::MAX),
            rows: u16::try_from(grid.rows).unwrap_or(u16::MAX),
            width_px: u16::try_from(grid.width_px).unwrap_or(u16::MAX),
            height_px: u16::try_from(grid.height_px).unwrap_or(u16::MAX),
        };
        let record = pane.record.unwrap_or_default();
        let adopted = shared.lock().adopt(
            record,
            grid,
            master,
            pane.process,
            &replay,
            pane.detection.as_ref(),
        );
        if let Err(problem) = adopted {
            return refuse(link, problem);
        }
    }
    Ok(())
}

/// Tells the daemon handing over why this one will not take over, and says the same here.
fn refuse<T>(link: &mut UnixStream, reason: String) -> Result<T, String> {
    log::warn(
        "daemon.handoff.refused",
        fields! {
            "reason" => reason,
            "impact" => "every pane stays with the daemon handing over, which goes on serving",
        },
    );
    let _ = send(link, handoff::Message::Refused(handoff::Refused { reason: reason.clone() }));
    Err(reason)
}
