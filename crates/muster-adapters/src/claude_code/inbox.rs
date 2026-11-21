//! Claude Code's inbox socket, as a wake adapter (MIP-4, section 6): one line carrying the
//! notice, on a connection opened only once the line is ready, since Claude Code closes one that
//! sends nothing for 30 seconds. No auth line: from a process that is not the session's child
//! the token changes nothing (`docs/observations/claude-code-2.1.283.md`), so the daemon never
//! holds it.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const SOCKET: &str = "CLAUDE_CODE_MESSAGING_SOCKET";

pub const WAKE_FAILURE_CHECK: &str = "whether its Claude Code session has exited; a session \
                                    that is still running and refuses its inbox is a bug";

/// How long a write to an inbox may take. A session's inbox is served by its own event loop on
/// this machine, so a line that takes longer is one that is not being read.
const PATIENCE: Duration = Duration::from_secs(1);

pub fn discover(environment: &BTreeMap<String, String>) -> Option<&str> {
    environment.get(SOCKET).filter(|socket| !socket.is_empty()).map(String::as_str)
}

pub fn inode(socket: &str) -> std::io::Result<u64> {
    std::fs::metadata(socket).map(|metadata| metadata.ino())
}

/// A new session can bind an old session's path after the system reuses its process id.
fn is_same_socket(socket: &str, expected_inode: u64) -> bool {
    inode(socket).is_ok_and(|inode| inode == expected_inode)
}

/// Hands the session one message, returning why it could not.
pub fn deliver(socket: &str, expected_inode: u64, text: &str) -> Result<(), String> {
    if !is_same_socket(socket, expected_inode) {
        return Err(format!("{socket} is gone, or belongs to another session now"));
    }
    let mut connection = connect_within(Path::new(socket), PATIENCE)
        .map_err(|error| format!("{socket}: {error}"))?;
    connection.set_write_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    let message = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    let mut line = message.to_string();
    line.push('\n');
    connection.write_all(line.as_bytes()).map_err(|error| format!("{socket}: {error}"))
}

/// Connecting without sending a line leaves nothing in the session's conversation.
pub fn alive(socket: &str, expected_inode: u64) -> bool {
    is_same_socket(socket, expected_inode) && connect_within(Path::new(socket), PATIENCE).is_ok()
}

/// Connects to a session's inbox, giving up after `patience` rather than waiting on it.
///
/// `UnixStream::connect` has no deadline, and on Linux a connect to a socket whose listen queue is
/// full blocks until the session accepts - which a stopped session never does, and the probes
/// themselves are what fill its queue. A probe holding up the message service for as long as a
/// session is stopped is every post and read on the machine stopping with it. macOS refuses such
/// a connect instead, so there this changes nothing.
fn connect_within(path: &Path, patience: Duration) -> std::io::Result<UnixStream> {
    // SAFETY: a plain-old-data C struct, valid zeroed.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the socket path is too long for a unix socket",
        ));
    }
    address.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).unwrap_or_default();
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = libc::c_char::from_ne_bytes([*from]);
    }
    #[cfg(target_os = "macos")]
    {
        address.sun_len = u8::try_from(size_of::<libc::sockaddr_un>()).unwrap_or(u8::MAX);
    }
    // SAFETY: socket returns a new descriptor or -1, and the descriptor is owned at once.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` is a descriptor this function just made and nothing else holds.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    set_flag(&socket, libc::F_GETFD, libc::F_SETFD, libc::FD_CLOEXEC, true)?;
    set_flag(&socket, libc::F_GETFL, libc::F_SETFL, libc::O_NONBLOCK, true)?;
    let length =
        libc::socklen_t::try_from(size_of::<libc::sockaddr_un>()).unwrap_or(libc::socklen_t::MAX);
    // SAFETY: the address is a valid sockaddr_un of `length` bytes, alive for the call.
    let connected =
        unsafe { libc::connect(socket.as_raw_fd(), std::ptr::from_ref(&address).cast(), length) };
    if connected == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        wait_until_connected(&socket, patience)?;
    }
    set_flag(&socket, libc::F_GETFL, libc::F_SETFL, libc::O_NONBLOCK, false)?;
    Ok(UnixStream::from(socket))
}

/// Waits for a connect in progress to finish, and says how it ended.
fn wait_until_connected(socket: &OwnedFd, patience: Duration) -> std::io::Result<()> {
    let mut waiting = libc::pollfd { fd: socket.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
    let millis = libc::c_int::try_from(patience.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid pollfd, alive for the call.
    let ready = unsafe { libc::poll(&raw mut waiting, 1, millis) };
    if ready == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if ready == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("the session did not accept a connection within {patience:?}"),
        ));
    }
    let mut failure: libc::c_int = 0;
    let mut length =
        libc::socklen_t::try_from(size_of::<libc::c_int>()).unwrap_or(libc::socklen_t::MAX);
    // SAFETY: SO_ERROR writes one c_int into `failure`, whose size `length` says.
    let asked = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            std::ptr::from_mut(&mut failure).cast(),
            &raw mut length,
        )
    };
    if asked == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if failure != 0 {
        return Err(std::io::Error::from_raw_os_error(failure));
    }
    Ok(())
}

/// Sets or clears one flag of a descriptor, through the `get`/`set` pair of `fcntl` commands.
fn set_flag(
    socket: &OwnedFd,
    get: libc::c_int,
    set: libc::c_int,
    flag: libc::c_int,
    on: bool,
) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this function's caller owns, reading and writing its flags.
    let flags = unsafe { libc::fcntl(socket.as_raw_fd(), get) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let flags = if on { flags | flag } else { flags & !flag };
    // SAFETY: as above.
    if unsafe { libc::fcntl(socket.as_raw_fd(), set, flags) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn socket_path(name: &str) -> std::path::PathBuf {
        let path =
            std::path::PathBuf::from(format!("/tmp/muster-inbox-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_listening_inbox_is_reached_and_a_gone_one_is_not() {
        let path = socket_path("listening");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("binds");
        assert!(connect_within(&path, PATIENCE).is_ok());
        drop(listener);
        let _ = std::fs::remove_file(&path);
        assert!(connect_within(&path, PATIENCE).is_err());
    }

    #[test]
    fn a_wake_writes_one_line_to_the_socket_it_was_addressed_to() {
        let path = socket_path("wake");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("binds");
        listener.set_nonblocking(true).unwrap();
        let socket = path.to_str().unwrap();
        let socket_inode = inode(socket).unwrap();
        let stale_inode = socket_inode.wrapping_add(1);
        assert!(!alive(socket, stale_inode));
        assert_eq!(
            deliver(socket, stale_inode, "stale").unwrap_err(),
            format!("{socket} is gone, or belongs to another session now")
        );

        deliver(socket, socket_inode, "first\nsecond").unwrap();
        let (mut arrived, _) = listener.accept().unwrap();
        arrived.set_nonblocking(true).unwrap();
        let mut line = String::new();
        arrived.read_to_string(&mut line).unwrap();
        assert_eq!(
            line,
            concat!(r#"{"message":{"content":"first\nsecond","role":"user"},"type":"user"}"#, "\n")
        );
        drop(listener);
        std::fs::remove_file(&path).unwrap();
    }

    /// A session that stops accepting, with its queue full, is not there - and asking says so
    /// at once rather than waiting until it accepts, which on Linux a plain connect does.
    #[test]
    fn a_session_whose_queue_is_full_is_answered_without_waiting() {
        let path = socket_path("full");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("binds");
        // SAFETY: listen on the listener's own descriptor, shrinking its queue to one.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let (done, finished) = std::sync::mpsc::channel();
        let probing = path.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            let mut refused = false;
            for _ in 0..16 {
                if let Ok(connection) = connect_within(&probing, PATIENCE) {
                    held.push(connection);
                } else {
                    refused = true;
                    break;
                }
            }
            let _ = done.send(refused);
        });
        let refused = finished
            .recv_timeout(Duration::from_secs(10))
            .expect("a probe of a full queue waited instead of answering");
        assert!(refused, "sixteen connections queued on a queue of one and none was refused");
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
