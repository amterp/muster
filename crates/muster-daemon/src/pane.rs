//! One pane: what the daemon publishes about it, and the PTY it runs on.
//!
//! [`Pane::start`] is the only way a pane comes to exist, and it takes a PTY master and an
//! optional child rather than spawning anything. A pane this daemon started has its child. A
//! pane handed over by a daemon being replaced (MIP-3, section 10) will arrive as a master and
//! its record with no child, because its process is some other daemon's child - so nothing about
//! a pane depends on this process being the PTY's parent.

use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;

use crate::process;
use crate::pty::{self, Grid};

/// Told when a pane's process has ended, with the pane's serial and, when this daemon saw the
/// process end, how it ended.
pub(crate) type Ended = Arc<dyn Fn(u64, Option<i32>) + Send + Sync>;

#[derive(Debug)]
pub(crate) struct Pane {
    /// What the protocol says about this pane. The fields a restart needs are persisted from
    /// here (a later card); the rest are observations.
    pub(crate) record: proto::Pane,
    pub(crate) grid: Grid,
    /// Tells this pane's process apart from a later pane given the same name.
    pub(crate) serial: u64,
    master: Arc<OwnedFd>,
    /// The write end of a pipe the reader also polls. Dropping it ends the reader, which is how
    /// a pane lets go of its master without closing a descriptor another thread is reading.
    wake: OwnedFd,
    /// The process this daemon started, when it started one: the leader of its own session.
    process: Option<i32>,
}

impl Pane {
    /// Starts watching `master`: a reader that drains it and, when there is a child, a waiter
    /// that reaps it and says how it ended.
    ///
    /// With a child, the child ending is what ends the pane, even if a background job still
    /// holds the terminal. Without one, the PTY closing is the only word there will be.
    pub(crate) fn start(
        record: proto::Pane,
        grid: Grid,
        serial: u64,
        master: OwnedFd,
        child: Option<Child>,
        ended: &Ended,
    ) -> io::Result<Pane> {
        let (wake_read, wake) = pipe()?;
        let master = Arc::new(master);
        let process = child.as_ref().map(|child| child.id().cast_signed());

        let reading = Arc::clone(&master);
        let reader_ends_pane = child.is_none().then(|| Arc::clone(ended));
        let pane = record.pane.clone();
        std::thread::Builder::new()
            .name(format!("read {pane}"))
            .spawn(move || drain(&reading, &wake_read, serial, reader_ends_pane.as_ref()))?;

        if let Some(mut child) = child {
            let ended = Arc::clone(ended);
            std::thread::Builder::new().name(format!("wait {pane}")).spawn(move || {
                let status = match child.wait() {
                    Ok(status) => exit_code(status),
                    Err(error) => {
                        log::error(
                            "daemon.pane.wait_failed",
                            fields! {
                                "pane" => pane,
                                "error" => error,
                                "impact" => "the pane is closed without an exit status, and its \
                                             process may be left unreaped",
                            },
                        );
                        None
                    }
                };
                ended(serial, status);
            })?;
        }

        Ok(Pane { record, grid, serial, master, wake, process })
    }

    /// The directory the pane is working in now: its foreground job's, else its shell's.
    pub(crate) fn live_cwd(&self) -> Option<PathBuf> {
        pty::foreground_group(self.master.as_fd())
            .and_then(process::cwd)
            .or_else(|| self.process.and_then(process::cwd))
    }

    /// Ends the pane: SIGHUP to its shell's process group and to whatever holds its terminal's
    /// foreground, then its master closed once the reader lets go of it.
    pub(crate) fn hang_up(self) {
        let foreground =
            pty::foreground_group(self.master.as_fd()).filter(|group| Some(*group) != self.process);
        for group in self.process.into_iter().chain(foreground) {
            pty::hang_up(group);
        }
        drop(self.wake);
    }
}

/// How a process ended, as a shell reports it in `$?`: its exit code, or 128 plus the signal
/// that ended it.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|signal| 128 + signal))
}

/// Reads the pane's output until its PTY closes or the pane lets go.
///
/// Discarded for now. The pane's byte stream and its headless terminal (later cards) take each
/// chunk here, and nothing on this path waits for the session lock.
fn drain(master: &OwnedFd, wake: &OwnedFd, serial: u64, ended: Option<&Ended>) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let mut watched = [
            libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: `watched` is a valid array of two pollfds for the length given.
        let ready = unsafe { libc::poll(watched.as_mut_ptr(), 2, -1) };
        if ready == -1 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if watched[1].revents != 0 {
            return;
        }
        if watched[0].revents == 0 {
            continue;
        }
        // SAFETY: `buffer` is valid for writes of its length.
        let read =
            unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            continue;
        }
        if read == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        // End of file, or EIO: every process holding the terminal has closed it.
        if let Some(ended) = ended {
            ended(serial, None);
        }
        return;
    }
}

/// A pipe whose two ends are close-on-exec, made under the session lock like every other
/// descriptor a pane owns (`pty.rs` says why that matters).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [-1; 2];
    // SAFETY: `ends` has room for the two descriptors pipe writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe succeeded, so both are open descriptors this function now owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    for end in [&read, &write] {
        // SAFETY: fcntl on a descriptor this function owns.
        if unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}
