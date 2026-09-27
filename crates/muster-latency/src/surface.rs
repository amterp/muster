//! What stands where a surface would: a PTY this process reads, with the real bridge or a bare
//! `cat` on the other end.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::daemon::GRID;
use crate::glyph;

/// A PTY at the measured grid, its master for this process and its replica for a child.
fn pty() -> (File, OwnedFd) {
    let (mut master, mut replica) = (0, 0);
    let narrow = |value: u32| u16::try_from(value).expect("the grid fits a winsize");
    let mut size = libc::winsize {
        ws_row: narrow(GRID.rows),
        ws_col: narrow(GRID.cols),
        ws_xpixel: narrow(GRID.width_px),
        ws_ypixel: narrow(GRID.height_px),
    };
    // SAFETY: openpty writes two descriptors we then own, and reads the winsize we own.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut replica,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: both were just opened and belong to nobody else.
    unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(replica)) }
}

/// Runs `command` with the replica as its terminal, and returns the master.
fn on_pty(command: &mut Command) -> (File, Owned) {
    let (master, replica) = pty();
    let child = command
        .stdin(Stdio::from(replica.try_clone().expect("a replica")))
        .stdout(Stdio::from(replica.try_clone().expect("a replica")))
        .stderr(Stdio::from(replica))
        .spawn()
        .unwrap_or_else(|error| panic!("could not start {command:?}: {error}"));
    (master, Owned(child))
}

/// Waits up to `within` for the master to have something to read.
fn readable(master: &File, within: Duration) -> bool {
    let mut watched = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let millis = i32::try_from(within.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd, for the length given.
    unsafe { libc::poll(&raw mut watched, 1, millis) > 0 }
}

/// A surface: the master end of a PTY, and what is on the other end.
pub(crate) struct Surface {
    master: File,
    child: Owned,
    /// Bytes read since the last [`Surface::take_bytes`].
    read: u64,
}

impl Surface {
    /// `cat` with nothing between it and the reader: the floor.
    pub(crate) fn plain() -> Surface {
        let (master, child) = on_pty(&mut Command::new("cat"));
        let mut surface = Surface { master, child, read: 0 };
        surface.settle();
        surface
    }

    /// The real bridge drawing `pane` from the daemon on `socket`.
    pub(crate) fn bridge(bridge: &Path, socket: &Path, pane: &str, log: &Path) -> Surface {
        let (master, child) = on_pty(
            Command::new(bridge)
                .arg(pane)
                .arg("--daemon-socket")
                .arg(socket)
                .env("MUSTER_LOG_FILE", log),
        );
        let mut surface = Surface { master, child, read: 0 };
        surface.settle();
        surface
    }

    /// Reads what has arrived until nothing has for a moment.
    pub(crate) fn settle(&mut self) {
        let mut buffer = vec![0u8; 65536];
        while readable(&self.master, Duration::from_millis(50)) {
            if self.master.read(&mut buffer).unwrap_or(0) == 0 {
                break;
            }
        }
        self.read = 0;
    }

    /// Writes `letter` into the PTY, as a key typed into the bare one.
    pub(crate) fn type_letter(&mut self, letter: u8) {
        self.master.write_all(&[letter]).expect("typing into the PTY");
    }

    /// How long until `letter` shows on the surface, timed from `from`.
    pub(crate) fn wait_for(&mut self, letter: u8, from: Instant, timeout: Duration) -> Option<f64> {
        let mut buffer = vec![0u8; 65536];
        let deadline = from + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            if !readable(&self.master, left) {
                return None;
            }
            let read = self.master.read(&mut buffer).ok().filter(|&read| read > 0)?;
            self.read += read as u64;
            if glyph::shows(&buffer[..read], letter) {
                return Some(from.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }

    pub(crate) fn take_bytes(&mut self) -> u64 {
        std::mem::take(&mut self.read)
    }

    /// Hands the master to a thread that reads it as `pace` says, keeping what it read, and
    /// returns what it has read so far on demand.
    pub(crate) fn read_in_background(self, pace: Pace) -> Background {
        let read = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&read);
        let Surface { mut master, child, .. } = self;
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; pace.chunk];
            let mut since_stall = 0;
            while let Ok(count) = master.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                if pace.keep {
                    kept.lock().expect("the read bytes").extend_from_slice(&buffer[..count]);
                }
                since_stall += count;
                if let Some((every, stall)) = pace.stall
                    && since_stall >= every
                {
                    since_stall = 0;
                    std::thread::sleep(stall);
                }
                std::thread::sleep(pace.pause);
            }
        });
        Background { _child: child, read }
    }
}

/// How a background reader reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pace {
    pub(crate) chunk: usize,
    pub(crate) pause: Duration,
    /// Every so many bytes, stop reading for this long.
    pub(crate) stall: Option<(usize, Duration)>,
    /// Whether to keep what was read.
    pub(crate) keep: bool,
}

impl Pace {
    /// As fast as it comes, discarded: a hidden pane's surface.
    pub(crate) const DRAIN: Pace =
        Pace { chunk: 65536, pause: Duration::ZERO, stall: None, keep: false };
}

/// A surface read by a thread of its own.
pub(crate) struct Background {
    _child: Owned,
    read: Arc<Mutex<Vec<u8>>>,
}

impl Background {
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.read.lock().expect("the read bytes").clone()
    }
}

/// A child ended with whatever holds it.
pub(crate) struct Owned(Child);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
