//! muster-daemon: the process that owns every pane's PTY on one machine, and outlives the app
//! (MIP-3).
//!
//! It runs in the foreground. Whoever starts it - Launch Services on the Mac, `setsid` over ssh,
//! a test harness - decides how it is detached; this binary only serves its socket until it is
//! told to stop.

mod control;
mod descriptors;
mod effects;
mod pane;
mod process;
mod pty;
mod screen;
mod server;
mod session;
mod spawn;
mod tree;
mod writer;

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::install;

use crate::session::Shared;

/// Exit status of a daemon that found another already serving its socket. Whoever started it
/// dials that one instead.
const ALREADY_SERVING: u8 = 3;

const USAGE: &str = "usage: muster-daemon [--socket PATH]\n\n\
    Serves Muster's panes on this machine. Without --socket, listens where this install's \
    daemon listens: $MUSTER_HOME/daemon/<install>.sock.";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let mut socket = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--socket" => match arguments.next() {
                Some(path) => socket = Some(PathBuf::from(path)),
                None => return usage("--socket needs a path"),
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

    log::start_from_environment("daemon");
    // Before any thread exists, so every thread inherits the mask and only the one waiting for
    // these signals ever receives them.
    let signals = block_signals();
    match run(&socket, signals) {
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

fn run(socket: &Path, signals: libc::sigset_t) -> Result<(), Failure> {
    refuse_anything_but_a_socket(socket)?;
    let _claim = claim(socket)?;
    let listener = listen(socket)?;

    let (stopping, stop) = mpsc::channel();
    let inherited: Vec<_> = std::env::vars_os().collect();
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    let shared = Shared::new(instance(), stopping.clone(), inherited, home);

    let waiting = signals;
    std::thread::Builder::new()
        .name("signals".to_string())
        .spawn(move || wait_for_signals(&waiting, &stopping))
        .map_err(|error| Failure::Other(format!("could not start the signal thread: {error}")))?;
    let accepting = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("accept".to_string())
        .spawn(move || server::accept(&listener, &accepting))
        .map_err(|error| Failure::Other(format!("could not start the accept thread: {error}")))?;

    log::info(
        "daemon.started",
        fields! {
            "socket" => socket.display(),
            "version" => env!("CARGO_PKG_VERSION"),
            "install" => install::INSTALL,
        },
    );
    let _ = stop.recv();
    shared.lock().close_everything();
    let _ = std::fs::remove_file(socket);
    log::info("daemon.stopped", fields! { "socket" => socket.display() });
    Ok(())
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
fn instance() -> u64 {
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

fn wait_for_signals(signals: &libc::sigset_t, stopping: &mpsc::Sender<()>) {
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
        let _ = stopping.send(());
        return;
    }
}
