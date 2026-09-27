//! Muster's side of muster-daemon's protocol (MIP-3, section 11): the control connection the
//! app asks and follows a daemon on, the input connection its keystrokes travel, and the stream
//! a bridge draws a pane from.

pub mod control;
pub mod input;
pub mod launch;
pub mod remote;
pub mod stream;

use std::os::unix::net::UnixStream;
use std::path::Path;

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{ConnectionKind, Welcome};

/// Dials a daemon and opens a connection of `kind`, on a socket whose writes cannot kill the
/// process.
///
/// The app hosts these connections, and a Swift process does not ignore SIGPIPE the way a Rust
/// binary does: a write to a daemon that has just died would end the window, and every pane's
/// surface with it.
fn dial(
    socket: &Path,
    kind: ConnectionKind,
    client: &str,
) -> Result<(UnixStream, Welcome), HandshakeError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        HandshakeError::Unreachable(format!("could not connect to {}: {error}", socket.display()))
    })?;
    silence_sigpipe(&stream);
    let welcome = connection::open(&mut stream, kind, client)?;
    Ok((stream, welcome))
}

#[cfg(target_vendor = "apple")]
fn silence_sigpipe(stream: &UnixStream) {
    use std::os::fd::AsRawFd;
    let on: libc::c_int = 1;
    // SAFETY: the fd is owned by `stream` and outlives the call; the option value is an int of
    // the size reported.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            std::ptr::from_ref(&on).cast(),
            u32::try_from(size_of::<libc::c_int>()).expect("an int fits a socklen"),
        );
    }
}

/// Linux has no such socket option, only a flag on each send; nothing hosts these connections
/// there but Rust binaries, which ignore SIGPIPE.
#[cfg(not(target_vendor = "apple"))]
fn silence_sigpipe(_stream: &UnixStream) {}
