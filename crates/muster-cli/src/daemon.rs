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
    let client = format!("muster {}", env!("CARGO_PKG_VERSION"));
    connection::connect(socket, kind, &client).map(|(stream, _)| stream).map_err(
        |error| match error {
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
        },
    )
}
