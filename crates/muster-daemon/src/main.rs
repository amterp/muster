//! muster-daemon: the process that owns every pane's PTY on one machine, and outlives the app
//! (MIP-3).
//!
//! It runs in the foreground. Whoever starts it - Launch Services on the Mac, `setsid` over ssh,
//! a test harness - decides how it is detached; this binary only serves its socket until it is
//! told to stop.

mod control;
mod daemon_log;
mod data;
mod descriptors;
mod detect;
mod effects;
mod facts;
mod handoff;
mod hold;
mod input;
mod pane;
mod persist;
mod process;
mod pty;
mod replace;
mod report;
mod screen;
mod server;
mod session;
mod shell_integration;
mod spawn;
mod stream;
mod tree;
mod writer;

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::install;

use crate::daemon_log::DaemonLog;
use crate::persist::Persister;
use crate::server::Socket;
use crate::session::{Places, Saved, Shared, Stop};

// musl's own allocator serializes every allocation on one lock, and the daemon allocates from a
// thread per pane (MIP-3, section 12). macOS's allocator does not have that problem.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Exit status of a daemon that found another already serving its socket. Whoever started it
/// dials that one instead.
const ALREADY_SERVING: u8 = 3;

/// How long a stopping daemon waits for its state to be written before it exits anyway.
const LAST_WRITE: Duration = Duration::from_secs(5);

const USAGE: &str = "usage: muster-daemon [--socket PATH] [--data DIR]\n       muster-daemon report ...\n       \
    muster-daemon replace ...\n\n\
    Serves Muster's panes on this machine. Without --socket, listens where this install's \
    daemon listens: $MUSTER_HOME/daemon/<install>.sock. Its log and its saved tabs are beside \
    the socket, as <name>.log and <name>.state.json; MUSTER_LOG=0 turns the log off. Without \
    --data, gives its shells the muster-daemon-data directory beside its executable. `report` \
    tells the daemon of the pane it runs in what the agent there says about itself; \
    `muster-daemon report --help` says how. `replace` hands a running daemon's panes to another \
    daemon without ending any; `muster-daemon replace --help` says how.";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("report") => return report::run(std::env::args().skip(2)),
        Some("replace") => return replace::run(std::env::args().skip(2)),
        _ => {}
    }
    let mut arguments = std::env::args().skip(1);
    let mut socket = None;
    let mut data = None;
    let mut handoff = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--socket" => match arguments.next() {
                Some(path) => socket = Some(PathBuf::from(path)),
                None => return usage("--socket needs a path"),
            },
            "--data" => match arguments.next() {
                Some(path) => data = Some(PathBuf::from(path)),
                None => return usage("--data needs a directory"),
            },
            // Given only by a daemon starting its successor (`handoff.rs`).
            "--handoff" => match arguments.next().and_then(|fd| fd.parse::<i32>().ok()) {
                Some(fd) => handoff = Some(fd),
                None => return usage("--handoff needs a descriptor number"),
            },
            "--version" => {
                println!("muster-daemon {} ({})", env!("CARGO_PKG_VERSION"), install::INSTALL);
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("{other} is not an option")),
        }
    }
    let Some(socket) = socket.or_else(|| {
        install::muster_home(|name| std::env::var(name).ok())
            .map(|home| install::socket_path(&home))
    }) else {
        return usage("neither MUSTER_HOME nor HOME is set, so there is no default socket");
    };

    // Before any thread exists, so every thread inherits the mask and only the one waiting for
    // these signals ever receives them.
    let signals = block_signals();
    let ran = match handoff {
        Some(link) => take_over(link, &socket, data.as_deref(), signals),
        None => run(&socket, data.as_deref(), signals),
    };
    match ran {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::AlreadyServing) => {
            eprintln!("muster-daemon: another daemon is already serving {}", socket.display());
            ExitCode::from(ALREADY_SERVING)
        }
        Err(Failure::Other(message)) => {
            eprintln!("muster-daemon: {message}");
            log::error("daemon.failed", fields! { "error" => message });
            ExitCode::FAILURE
        }
    }
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("muster-daemon: {problem}\n{USAGE}");
    ExitCode::from(2)
}

enum Failure {
    AlreadyServing,
    Other(String),
}

fn run(socket: &Path, data: Option<&Path>, signals: libc::sigset_t) -> Result<(), Failure> {
    refuse_anything_but_a_socket(socket)?;
    let claim = claim(socket)?;
    // Only once the socket is this daemon's: the log beside it has one writer, and a second
    // daemon turned away must not touch it.
    let log = DaemonLog::start(socket, false);
    // Logged as daemon.failed on the way out, with the message saying what is missing.
    let data = data::Data::locate(data).map_err(Failure::Other)?;
    let listener = listen(socket)?;
    let socket = Socket::new(socket.to_path_buf(), listener, claim).map_err(|error| {
        Failure::Other(format!("could not set up {}: {error}", socket.display()))
    })?;

    let (stopping, stop) = mpsc::channel();
    let inherited: Vec<_> = std::env::vars_os().collect();
    let places = Places::of_this_process(&socket.path, data, log);
    let (saved, state) = saved(&socket.path);
    let persister = Arc::clone(&saved.persister);
    let shared = Shared::new(instance(), stopping.clone(), inherited, places, saved, socket);
    shared.point_link();
    serve(&shared, signals, stopping).map_err(Failure::Other)?;

    // Once the socket is served: a shell starting in a directory on a hung mount must not keep
    // the daemon from answering.
    match state {
        Some(state) => {
            let restoring = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("restore".to_string())
                .spawn(move || session::restore(&restoring, state))
                .map_err(|error| {
                    Failure::Other(format!("could not start the restore thread: {error}"))
                })?;
        }
        None => persister.arm(),
    }

    log::info(
        "daemon.started",
        fields! {
            "socket" => shared.socket.path.display(),
            "version" => env!("CARGO_PKG_VERSION"),
            "install" => install::INSTALL,
        },
    );
    wait(&shared, &stop, &persister);
    Ok(())
}

/// Serves in place of the daemon that started this one to hand over its panes.
fn take_over(
    link: i32,
    socket: &Path,
    data: Option<&Path>,
    signals: libc::sigset_t,
) -> Result<(), Failure> {
    let taken = handoff::take_over(link, socket, data, signals).map_err(|why| {
        Failure::Other(format!("did not take over from the daemon on {}: {why}", socket.display()))
    })?;
    wait(&taken.shared, &taken.stop, &taken.persister);
    Ok(())
}

/// Starts waiting for signals and accepting connections: from here the daemon serves.
pub(crate) fn serve(
    shared: &Arc<Shared>,
    signals: libc::sigset_t,
    stopping: Sender<Stop>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("signals".to_string())
        .spawn(move || wait_for_signals(&signals, &stopping))
        .map_err(|error| format!("could not start the signal thread: {error}"))?;
    let accepting = Arc::clone(shared);
    std::thread::Builder::new()
        .name("accept".to_string())
        .spawn(move || server::accept(&accepting))
        .map_err(|error| format!("could not start the accept thread: {error}"))?;
    Ok(())
}

/// Serves until told to stop, then closes every pane, unless they were handed to another
/// daemon: that one serves the socket now, so nothing of it is touched on the way out.
fn wait(shared: &Shared, stop: &Receiver<Stop>, persister: &Persister) {
    let socket = &shared.socket.path;
    if stop.recv() == Ok(Stop::HandedOff) {
        return;
    }
    shared.lock().close_everything();
    persister.wait_until_stopped(LAST_WRITE);
    let _ = std::fs::remove_file(socket);
    log::info("daemon.stopped", fields! { "socket" => socket.display() });
}

/// Finds the state a previous run of this daemon saved, and what to write this run's with.
///
/// A file this daemon cannot read is never replaced: one from a newer daemon is left for that
/// daemon, and one that is damaged is moved aside, so that whatever it held can still be read
/// by a person. Either way the daemon starts, empty, rather than refusing to: a daemon that will
/// not start ends no agent, but it starts none either.
fn saved(socket: &Path) -> (Saved, Option<persist::State>) {
    let path = persist::path_for(socket);
    let (disabled, state) = match persist::load(&path) {
        persist::Loaded::Nothing => (false, None),
        persist::Loaded::State(state) => (false, Some(state)),
        persist::Loaded::Newer(version) => {
            let problem = format!(
                "{} was written by a newer muster-daemon (format {version}; this one reads up \
                 to {}), so this daemon starts with no tabs and saves nothing over it",
                path.display(),
                persist::VERSION
            );
            eprintln!("muster-daemon: {problem}");
            log::error(
                "daemon.state.newer",
                fields! {
                    "path" => path.display(),
                    "version" => version,
                    "impact" => problem,
                    "fix" => "run the newer daemon again to get its tabs back, or move the file \
                              away to let this one save its own",
                },
            );
            (true, None)
        }
        persist::Loaded::Corrupt(why) => match persist::move_aside(&path) {
            Ok(aside) => {
                log::warn(
                    "daemon.state.corrupt",
                    fields! {
                        "path" => path.display(),
                        "moved_to" => aside.display(),
                        "why" => why,
                        "impact" => "the daemon starts with no tabs; the file is kept where it \
                                     was moved",
                        "check" => "whether something other than the daemon edited the file, \
                                    or the disk it is on is failing",
                    },
                );
                (false, None)
            }
            Err(error) => {
                log::error(
                    "daemon.state.corrupt",
                    fields! {
                        "path" => path.display(),
                        "why" => why,
                        "error" => error,
                        "impact" => "the file could not be moved aside, so the daemon starts \
                                     with no tabs and saves nothing over it",
                        "check" => "the permissions on the file and its directory",
                    },
                );
                (true, None)
            }
        },
        persist::Loaded::Unreadable(error) => {
            log::error(
                "daemon.state.unreadable",
                fields! {
                    "path" => path.display(),
                    "error" => error,
                    "impact" => "the daemon starts with no tabs and saves nothing over the file",
                    "check" => "the permissions on the file and its directory",
                },
            );
            (true, None)
        }
    };
    let settings = state.as_ref().map(|state| state.settings.clone());
    let restoring = state.is_some();
    (Saved { persister: Persister::new(path, disabled), settings, restoring }, state)
}

/// Refuses a path that holds something other than a socket, because binding replaces what is
/// there: a mistyped `--socket` must not delete somebody's file.
fn refuse_anything_but_a_socket(socket: &Path) -> Result<(), Failure> {
    match std::fs::symlink_metadata(socket) {
        Ok(found) if !found.file_type().is_socket() => Err(Failure::Other(format!(
            "{} is not a socket, and listening there would replace it; name a path that is \
             free or holds a socket",
            socket.display()
        ))),
        _ => Ok(()),
    }
}

/// Takes the lock that makes this the one daemon on `socket`, held until the process exits.
///
/// A lock file beside the socket rather than a probe of the socket itself: two daemons started
/// at the same moment would both find nothing listening, and the second to bind would take the
/// socket from the first while the first went on running, unreachable. The kernel drops the lock
/// when the process ends, however it ends.
fn claim(socket: &Path) -> Result<File, Failure> {
    let directory = socket.parent().unwrap_or(Path::new("."));
    if !directory.exists() {
        std::fs::create_dir_all(directory).map_err(|error| {
            Failure::Other(format!("could not create {}: {error}", directory.display()))
        })?;
        // Only the directory this made: a socket's directory somebody chose is theirs to set.
        let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
    }
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    let lock =
        File::options().create(true).truncate(false).write(true).open(&path).map_err(|error| {
            Failure::Other(format!("could not open {}: {error}", Path::new(&path).display()))
        })?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
            Failure::AlreadyServing
        } else {
            Failure::Other(format!("could not lock {}: {error}", Path::new(&path).display()))
        });
    }
    Ok(lock)
}

/// Binds the socket, replacing one a daemon that ended without cleaning up left behind. Only
/// the holder of the claim gets here, so whatever is at that path is not serving anyone.
fn listen(socket: &Path) -> Result<UnixListener, Failure> {
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket).map_err(|error| {
        Failure::Other(format!("could not listen on {}: {error}", socket.display()))
    })?;
    // Anyone who can dial this socket can type into every pane, so it is the user's alone.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        Failure::Other(format!("could not restrict {}: {error}", socket.display()))
    })?;
    Ok(listener)
}

/// A number naming this run of the daemon, so a client can tell sequence numbers from two runs
/// apart. The clock and the pid together, because either alone repeats.
pub(crate) fn instance() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    #[allow(clippy::cast_possible_truncation)]
    let nanos = nanos as u64;
    nanos ^ (u64::from(std::process::id()) << 40)
}

/// Blocks the signals the daemon handles, and returns the set.
///
/// Blocked and waited for rather than ignored, and the difference matters: an ignored signal
/// stays ignored across exec, so a daemon ignoring SIGHUP would start every pane deaf to the
/// hangup that closing it sends. A blocked mask is inherited across fork too, which is why
/// `pty.rs` clears it in every pane before exec.
///
/// For the same reason, anything whoever started the daemon left ignored is put back to its
/// default first. `nohup` ignores SIGHUP, and a daemon started under it would otherwise hand that
/// on to every pane. SIGPIPE stays as std set it: the daemon needs it ignored, and std restores
/// it in every child.
fn block_signals() -> libc::sigset_t {
    for signal in 1..32 {
        if [libc::SIGKILL, libc::SIGSTOP, libc::SIGPIPE].contains(&signal) {
            continue;
        }
        // SAFETY: sigaction reads and writes only the struct given, and only replaces an ignore
        // with the default.
        unsafe {
            let mut current = std::mem::zeroed::<libc::sigaction>();
            if libc::sigaction(signal, std::ptr::null(), &raw mut current) == 0
                && current.sa_sigaction == libc::SIG_IGN
            {
                let mut default = std::mem::zeroed::<libc::sigaction>();
                default.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(signal, &raw const default, std::ptr::null_mut());
            }
        }
    }
    let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: sigemptyset initialises the set; sigaddset and pthread_sigmask only read and
    // write it and this thread's mask.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(set.as_mut_ptr(), signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut());
        set.assume_init()
    }
}

fn wait_for_signals(signals: &libc::sigset_t, stopping: &Sender<Stop>) {
    loop {
        let mut signal = 0;
        // SAFETY: sigwait reads the set and writes one signal number.
        if unsafe { libc::sigwait(signals, &raw mut signal) } != 0 {
            continue;
        }
        if signal == libc::SIGHUP {
            // The terminal that started a daemon by hand closing is no reason to end every pane.
            log::info("daemon.signal.ignored", fields! { "signal" => "SIGHUP" });
            continue;
        }
        log::info("daemon.signal.stopping", fields! { "signal" => signal });
        let _ = stopping.send(Stop::Asked);
        return;
    }
}
