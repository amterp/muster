//! Muster's side of muster-daemon's protocol (MIP-3, section 11): the control connection the
//! app asks and follows a daemon on, the input connection its keystrokes travel, the stream a
//! bridge draws a pane from, and starting or adopting a daemon here or on another machine.

pub mod backend;
pub mod control;
pub mod convert;
pub mod environment;
pub mod follow;
pub mod handover;
pub mod input;
pub mod install;
pub mod launch;
pub mod records;
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
/// process ([`silence_sigpipe`]). The handshake is bounded, so a daemon that accepts and never
/// answers fails the dial rather than holding its caller for ever.
fn dial(
    socket: &Path,
    kind: ConnectionKind,
    client: &str,
) -> Result<(UnixStream, Welcome), HandshakeError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        HandshakeError::Unreachable(format!("could not connect to {}: {error}", socket.display()))
    })?;
    silence_sigpipe(&stream).map_err(|error| {
        HandshakeError::Unreachable(format!(
            "could not stop a write to {} from ending this process ({error}), so it was not used",
            socket.display()
        ))
    })?;
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

/// A line a start writes to the stderr file it shares with any other start on that socket, so
/// that it can quote only what followed it, and the token its daemon repeats in its welcome, so
/// that it knows the daemon that answered is its own.
///
/// Counted as well as timed, because the clock alone does not tell two starts apart: macOS's
/// ticks in whole microseconds, and two threads of one process starting the same socket land
/// in the same one often enough that each took the other's daemon for its own.
fn start_marker() -> String {
    static STARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let start = STARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("--- muster-daemon start {}-{nanos}-{start} ---", std::process::id())
}

/// What a stderr file holds after `marker`'s line, trimmed.
fn after_marker<'a>(text: &'a str, marker: &str) -> &'a str {
    text.rfind(marker).map_or("", |at| text[at + marker.len()..].trim())
}

fn bounded(stream: &UnixStream, patience: Option<Duration>) -> std::io::Result<()> {
    stream.set_read_timeout(patience)?;
    stream.set_write_timeout(patience)
}

/// Makes a write to `stream` whose far end has gone fail with `EPIPE` rather than raise
/// SIGPIPE, which by default ends the process.
///
/// Needed because the app hosts these sockets in a Swift process, which does not ignore SIGPIPE
/// the way a Rust binary does: a write to a daemon, bridge or CLI that had just gone would end
/// the window and every pane's surface with it. macOS spells this as a socket option.
#[cfg(target_vendor = "apple")]
pub fn silence_sigpipe(stream: &UnixStream) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let on: libc::c_int = 1;
    // SAFETY: the fd is owned by `stream` and outlives the call; the option value is an int of
    // the size reported.
    let set = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            std::ptr::from_ref(&on).cast(),
            u32::try_from(size_of::<libc::c_int>()).expect("an int fits a socklen"),
        )
    };
    if set == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Linux has no such socket option, only a flag on each send, and nothing here needs one today:
/// every process that holds these sockets on Linux is a Rust binary (the daemon, the bridge, the
/// CLI, the tests), and Rust's runtime ignores SIGPIPE before `main`. libmuster, the one library
/// a foreign process loads, is loaded only by the macOS shell. A Linux shell that hosts it must
/// ignore SIGPIPE itself before opening any connection.
#[cfg(not(target_vendor = "apple"))]
pub fn silence_sigpipe(_stream: &UnixStream) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A marker is how a start knows the daemon that answered is its own, and two starts in one
    /// process - two threads, or one retrying - can begin inside the same tick of the clock.
    #[test]
    fn every_start_has_a_marker_of_its_own() {
        let markers: std::collections::HashSet<String> =
            (0..1000).map(|_| start_marker()).collect();
        assert_eq!(markers.len(), 1000);
    }
}
