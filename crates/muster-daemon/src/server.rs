//! Accepting connections, and the handshake that opens each one.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::version::{PROTOCOL, compatible};
use muster_daemon_proto::{self as proto, ConnectionKind, hello_answer, install};

use crate::control;
use crate::hold::{Hold, Leaving};
use crate::input;
use crate::session::Shared;
use crate::stream;

/// What whoever started this daemon asked it to repeat in its welcome (`--launch`).
static LAUNCH: OnceLock<String> = OnceLock::new();

pub(crate) fn set_launch(token: String) {
    let _ = LAUNCH.set(token);
}

/// How long a connection may take to say hello. Past it, whatever dialed is not a Muster client
/// and is not worth a thread.
const HELLO_PATIENCE: Duration = Duration::from_secs(5);

/// The daemon's socket: what listens on it, the lock that makes it this daemon's, and the hold a
/// handoff takes on accepting. The listener and the lock are what a handoff passes on.
#[derive(Debug)]
pub(crate) struct Socket {
    pub(crate) path: PathBuf,
    pub(crate) listener: UnixListener,
    pub(crate) lock: File,
    pub(crate) accepting: Hold,
}

impl Socket {
    /// Non-blocking, because a daemon handing over and the daemon taking over share one
    /// listener: a connection one of them was woken for may be accepted by the other first.
    pub(crate) fn new(path: PathBuf, listener: UnixListener, lock: File) -> io::Result<Socket> {
        listener.set_nonblocking(true)?;
        Ok(Socket { path, listener, lock, accepting: Hold::new(false)? })
    }
}

/// Points the link beside `socket`, `<socket stem>.muster-daemon`, at `executable`, replacing what
/// it named in one rename, and returns what a pane is told the daemon's executable is: the link,
/// or `executable` itself when the link could not be made. Each daemon points it at itself as it
/// starts to serve, a daemon taking over included.
pub(crate) fn point_link(socket: &Path, executable: &Path) -> PathBuf {
    let link = socket.with_extension("muster-daemon");
    let mut temporary = link.as_os_str().to_owned();
    temporary.push(format!(".{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let _ = std::fs::remove_file(&temporary);
    let pointed = std::os::unix::fs::symlink(executable, &temporary)
        .and_then(|()| std::fs::rename(&temporary, &link));
    match pointed {
        Ok(()) => link,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            log::warn(
                "daemon.link.not_pointed",
                fields! {
                    "link" => link.display(),
                    "executable" => executable.display(),
                    "error" => error,
                    "impact" => "panes started from now are told this executable's own path, \
                                 which a handoff to a daemon elsewhere leaves them naming",
                    "check" => "the permissions on the socket's directory",
                },
            );
            executable.to_path_buf()
        }
    }
}

/// Serves every connection to the daemon's socket, each on a thread of its own, for as long as
/// the daemon runs. While a handoff holds it, a connection waits in the listener's backlog for
/// whichever daemon serves next.
pub(crate) fn accept(shared: &Arc<Shared>) {
    let socket = &shared.socket;
    let _leaving = Leaving(&socket.accepting);
    loop {
        socket.accepting.park(|| false, || {});
        let mut watched = [
            libc::pollfd { fd: socket.listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: socket.accepting.polled(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: `watched` is a valid array of two pollfds for the length given.
        if unsafe { libc::poll(watched.as_mut_ptr(), 2, -1) } == -1 || watched[0].revents == 0 {
            continue;
        }
        let Ok((stream, _)) = socket.listener.accept() else { continue };
        // A socket accepted from a non-blocking listener is non-blocking itself on macOS.
        if stream.set_nonblocking(false).is_err() {
            continue;
        }
        let shared = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("connection".to_string())
            .spawn(move || open(stream, &shared));
        if let Err(error) = spawned {
            log::error(
                "daemon.connection.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "a client's connection was closed unanswered",
                    "check" => "the daemon's thread count; a leak shows as one per pane or client",
                },
            );
        }
    }
}

fn open(mut stream: UnixStream, shared: &Arc<Shared>) {
    let _ = stream.set_read_timeout(Some(HELLO_PATIENCE));
    let hello = match connection::receive::<proto::Hello>(&mut stream) {
        Ok(Some(hello)) => hello,
        Ok(None) => return,
        Err(error) => {
            log::warn(
                "daemon.connection.no_hello",
                fields! {
                    "error" => error,
                    "impact" => "the connection was closed; whatever dialed was not answered",
                    "check" => "whether something other than Muster is dialing the daemon's socket",
                },
            );
            return;
        }
    };
    let answer = match judge(&hello) {
        Ok(()) => hello_answer::Answer::Welcome(proto::Welcome {
            protocol: Some(PROTOCOL),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            install: install::INSTALL.to_string(),
            instance: shared.instance,
            pid: std::process::id(),
            launch: LAUNCH.get().cloned().unwrap_or_default(),
        }),
        Err(reason) => {
            log::warn(
                "daemon.connection.refused",
                fields! {
                    "client" => hello.client,
                    "reason" => reason,
                    "impact" => "that client cannot use this daemon",
                },
            );
            hello_answer::Answer::Refused(proto::HelloRefused { reason, daemon: Some(PROTOCOL) })
        }
    };
    let welcomed = matches!(answer, hello_answer::Answer::Welcome(_));
    let answer = proto::HelloAnswer { answer: Some(answer) };
    if connection::send(&mut stream, &answer).is_err() || !welcomed {
        return;
    }
    let _ = stream.set_read_timeout(None);
    match ConnectionKind::try_from(hello.kind) {
        Ok(ConnectionKind::Stream) => stream::serve(stream, shared),
        Ok(ConnectionKind::Input) => input::serve(stream, shared, &hello.client),
        _ => control::serve(stream, shared, &hello.client),
    }
}

/// Whether this daemon will serve a connection that says hello this way.
fn judge(hello: &proto::Hello) -> Result<(), String> {
    let Some(theirs) = hello.protocol else {
        return Err("the hello names no protocol version".to_string());
    };
    if !compatible(&PROTOCOL, &theirs) {
        return Err(format!(
            "this daemon speaks protocol {PROTOCOL} and the client speaks {theirs}; a client \
             talks only to a daemon of its own major version"
        ));
    }
    match ConnectionKind::try_from(hello.kind) {
        Ok(ConnectionKind::Control | ConnectionKind::Stream | ConnectionKind::Input) => Ok(()),
        Ok(ConnectionKind::Unspecified) | Err(_) => {
            Err("the hello names no kind of connection this daemon knows".to_string())
        }
    }
}
