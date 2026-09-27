//! The one writer to a pane's program.
//!
//! Keystrokes, pastes, `pane send` text and the answers to the program's own queries all reach
//! the program through one queue per pane, drained by one thread, in the order they were
//! queued. Nothing else writes to a pane's PTY. A queue that is full drops a query answer
//! rather than make the reader wait, which is what stops a program that floods output full of
//! queries while not reading its input from wedging the daemon.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, SyncSender};

use muster_core::diagnostics::log;
use muster_core::fields;

/// How much may wait for a pane's program to read it.
const QUEUE_DEPTH: usize = 1024;

/// Something for a pane's program to read.
#[derive(Debug)]
pub(crate) enum Input {
    /// Bytes as they are: a query's answer, or a report the program asked for.
    Reply(Vec<u8>),
}

pub(crate) fn queue() -> (SyncSender<Input>, Receiver<Input>) {
    mpsc::sync_channel(QUEUE_DEPTH)
}

/// Writes everything queued for a pane until the pane lets go of its queue, or of its PTY.
pub(crate) fn write(pane: &str, queued: &Receiver<Input>, master: &OwnedFd, wake: &OwnedFd) {
    for input in queued {
        let Input::Reply(bytes) = input;
        if let Err(error) = write_all(master, wake, &bytes) {
            if error.kind() != io::ErrorKind::BrokenPipe {
                log::warn(
                    "daemon.pane.write_failed",
                    fields! {
                        "pane" => pane,
                        "error" => error,
                        "impact" => "the pane's program stops receiving input; its pane is \
                                     closing or its terminal has gone",
                    },
                );
            }
            return;
        }
    }
}

/// Writes all of `bytes` to the master, waiting while the program is not reading. Gives up
/// with `BrokenPipe` once the pane lets go (its wake pipe closes), so a program that never
/// reads again cannot hold this thread.
fn write_all(master: &OwnedFd, wake: &OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: the slice is valid for reads of its length.
        let written =
            unsafe { libc::write(master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if written >= 0 {
            bytes = &bytes[written.cast_unsigned()..];
            continue;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => {
                if !writable(master, wake)? {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
            _ => return Err(error),
        }
    }
    Ok(())
}

/// Waits until the master takes more. False once the pane has let go.
fn writable(master: &OwnedFd, wake: &OwnedFd) -> io::Result<bool> {
    loop {
        let mut watched = [
            libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLOUT, revents: 0 },
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: `watched` is a valid array of two pollfds for the length given.
        if unsafe { libc::poll(watched.as_mut_ptr(), 2, -1) } == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if watched[1].revents != 0 {
            return Ok(false);
        }
        if watched[0].revents != 0 {
            return Ok(true);
        }
    }
}

/// A descriptor for the same pipe end, close-on-exec, so the writer can watch the wake pipe
/// beside the reader.
pub(crate) fn duplicate(fd: &OwnedFd) -> io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: F_DUPFD_CLOEXEC on a descriptor this process owns returns a new one or -1.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl just returned this descriptor, which nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}
