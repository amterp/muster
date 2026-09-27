//! Muster's side of muster-daemon's protocol (MIP-3, section 11): the control connection the
//! app asks and follows a daemon on, the input connection its keystrokes travel, the stream a
//! bridge draws a pane from, and starting or adopting a daemon here or on another machine.

pub mod control;
pub mod input;
pub mod launch;
pub mod remote;
pub mod stream;

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{ConnectionKind, Welcome};

/// How long a daemon has to finish the handshake. A daemon answers in milliseconds, and so does
/// one at the far end of an ssh forward; this is for one that accepted and then stalled.
const HANDSHAKE_PATIENCE: Duration = Duration::from_secs(5);

/// Dials a daemon and opens a connection of `kind`, on a socket whose writes cannot kill the
/// process. The handshake is bounded, so a daemon that accepts and never answers fails the dial
/// rather than holding its caller for ever.
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
    bounded(&stream, Some(HANDSHAKE_PATIENCE)).map_err(|error| {
        HandshakeError::Unreachable(format!("could not bound the handshake: {error}"))
    })?;
    let started = Instant::now();
    let welcome = match connection::open(&mut stream, kind, client) {
        Ok(welcome) => welcome,
        // A timeout reads as an error like any other, so the clock says which it was.
        Err(HandshakeError::Unreachable(why) | HandshakeError::Garbled(why))
            if started.elapsed() >= HANDSHAKE_PATIENCE =>
        {
            return Err(HandshakeError::Stalled(format!(
                "{} accepted the connection and did not answer within {}s ({why})",
                socket.display(),
                HANDSHAKE_PATIENCE.as_secs()
            )));
        }
        Err(error) => return Err(error),
    };
    // From here the connection's own reader waits as long as the daemon is quiet, which on a
    // control connection is most of the time.
    bounded(&stream, None).map_err(|error| {
        HandshakeError::Unreachable(format!("could not unbound the connection: {error}"))
    })?;
    Ok((stream, welcome))
}

fn bounded(stream: &UnixStream, patience: Option<Duration>) -> std::io::Result<()> {
    stream.set_read_timeout(patience)?;
    stream.set_write_timeout(patience)
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
