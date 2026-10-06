//! Reaching this machine's muster-daemon, rather than a window.
//!
//! `muster msg` always comes here, since messaging has to work with no window open, and the pane
//! verbs come here when no window answers (`windowless`). Both find the daemon the same way and
//! fail in the same words, except that `muster msg` starts this install's daemon when none runs
//! (`starting`).

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{ConnectionKind, install};

use crate::{Trouble, environment};

/// Names the daemon a pane runs on; every pane Muster makes has it.
pub const SOCKET: &str = "MUSTER_DAEMON_SOCKET";

/// `$MUSTER_DAEMON_SOCKET`, which names the daemon a pane runs on, else this install's daemon
/// under Muster's home.
pub fn socket(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(socket) = environment.get(SOCKET).filter(|socket| !socket.is_empty()) {
        return Some(PathBuf::from(socket));
    }
    environment::muster_home(environment).map(|home| install::socket_path(Path::new(&home)))
}

/// The same, or why there is none to ask.
pub fn socket_or_refusal(environment: &BTreeMap<String, String>) -> Result<PathBuf, Trouble> {
    socket(environment).ok_or_else(|| {
        Trouble::Unreachable(format!(
            "no muster-daemon to ask: ${SOCKET} is not set and neither is $HOME."
        ))
    })
}

/// A connection of `kind` to the daemon at `socket`, past its handshake.
pub fn connect(socket: &Path, kind: ConnectionKind) -> Result<UnixStream, Trouble> {
    connect_welcomed(socket, kind).map(|(stream, _)| stream)
}

/// [`connect`], with what the daemon said of itself: which protocol it speaks, among the rest.
pub fn connect_welcomed(
    socket: &Path,
    kind: ConnectionKind,
) -> Result<(UnixStream, muster_daemon_proto::Welcome), Trouble> {
    open(socket, kind).map_err(|missed| match missed {
        Missed::Nobody(error) => nobody_at(socket, &error),
        Missed::Otherwise(trouble) => trouble,
    })
}

/// What `muster msg` needs to start this machine's daemon when it finds none running.
#[derive(Debug, Clone, Copy)]
pub struct MayStart<'a> {
    pub environment: &'a BTreeMap<String, String>,
    pub json: bool,
}

/// [`connect_welcomed`], starting this machine's daemon first when nothing listens on `socket`
/// (MIP-4, section 1). Tried only after a connection fails, so a daemon that is running costs
/// nothing extra.
pub fn connect_or_start(
    socket: &Path,
    kind: ConnectionKind,
    may: MayStart,
) -> Result<(UnixStream, muster_daemon_proto::Welcome), Trouble> {
    match open(socket, kind) {
        Ok(connected) => Ok(connected),
        Err(Missed::Otherwise(trouble)) => Err(trouble),
        Err(Missed::Nobody(error)) => {
            crate::starting::start(socket, &error, may)?;
            connect_welcomed(socket, kind)
        }
    }
}

/// Why a connection was not made.
enum Missed {
    /// Nothing listens there: no socket file, or one a daemon left behind when it ended.
    Nobody(std::io::Error),
    Otherwise(Trouble),
}

fn open(
    socket: &Path,
    kind: ConnectionKind,
) -> Result<(UnixStream, muster_daemon_proto::Welcome), Missed> {
    let client = format!("muster {}", env!("CARGO_PKG_VERSION"));
    // Connected here rather than by `connection::connect`, which keeps only the error's words:
    // a sandbox refusing the socket is not a daemon that is not running.
    let mut stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            Missed::Nobody(error)
        }
        std::io::ErrorKind::PermissionDenied => {
            Missed::Otherwise(Trouble::Unreachable(crate::dial::not_permitted(
                &format!("the muster-daemon at {}", socket.display()),
                &error,
            )))
        }
        _ => Missed::Otherwise(nobody_at(socket, &error)),
    })?;
    let welcome = connection::open(&mut stream, kind, &client).map_err(|error| {
        Missed::Otherwise(match error {
            HandshakeError::Refused(refused) => Trouble::Refused(format!(
                "the muster-daemon at {} would not talk to this muster: {}",
                socket.display(),
                refused.reason
            )),
            HandshakeError::Unreachable(why)
            | HandshakeError::Garbled(why)
            | HandshakeError::Stalled(why) => Trouble::Unreachable(format!(
                "no muster-daemon answered at {} ({why}). One runs while Muster does; set \
                 ${SOCKET} to reach another.",
                socket.display()
            )),
        })
    })?;
    Ok((stream, welcome))
}

fn nobody_at(socket: &Path, error: &std::io::Error) -> Trouble {
    Trouble::Unreachable(format!(
        "no muster-daemon answered at {} ({error}). One runs while Muster does; set ${SOCKET} \
         to reach another.",
        socket.display()
    ))
}
