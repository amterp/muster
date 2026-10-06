//! Reaching this machine's muster-daemon, rather than a window.
//!
//! `muster msg` always comes here, since messaging has to work with no window open, and the pane
//! verbs come here when no window answers (`windowless`). Both find the daemon the same way and
//! fail in the same words.

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
    let client = format!("muster {}", env!("CARGO_PKG_VERSION"));
    // Connected here rather than by `connection::connect`, which keeps only the error's words:
    // a sandbox refusing the socket is not a daemon that is not running.
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        let what = format!("the muster-daemon at {}", socket.display());
        Trouble::Unreachable(if error.kind() == std::io::ErrorKind::PermissionDenied {
            crate::dial::not_permitted(&what, &error)
        } else {
            format!(
                "no muster-daemon answered at {} ({error}). One runs while Muster does; set \
                 ${SOCKET} to reach another.",
                socket.display()
            )
        })
    })?;
    let welcome = connection::open(&mut stream, kind, &client).map_err(|error| match error {
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
    })?;
    Ok((stream, welcome))
}
