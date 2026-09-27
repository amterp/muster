//! What the bridge tells the window, on the socket the window bound for its pane.
//!
//! The window learns a bridge died from this socket closing, so it is dialled once, as soon
//! as the pane's stream is attached, and held for the bridge's whole life
//! (`muster_core::bridge_link`).

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use muster_core::bridge_link::{PAINTED_INTERVAL_NS, Report};
use muster_core::diagnostics::{log, monotonic_now, poison};
use muster_core::fields;

use crate::tally::{Counted, Tally};

/// The window's end of the link, or nothing when the bridge was started without one (as the
/// latency tier starts it) or the window could not be reached.
#[derive(Clone, Default)]
pub(crate) struct Link {
    socket: Option<Arc<Mutex<UnixStream>>>,
}

impl Link {
    pub(crate) fn dial(path: Option<&str>) -> Link {
        let Some(path) = path else { return Link::default() };
        let dialled = UnixStream::connect(path).and_then(|stream| {
            muster_daemon_client::silence_sigpipe(&stream)?;
            Ok(stream)
        });
        match dialled {
            Ok(stream) => Link { socket: Some(Arc::new(Mutex::new(stream))) },
            Err(error) => {
                log::warn(
                    "bridge.link.failed",
                    fields! {
                        "socket" => path,
                        "error" => error,
                        "impact" => "the pane draws, and the window does not know this bridge \
                                     is here: it reports the pane as waiting for a bridge and \
                                     starts another",
                        "check" => "whether the window is still open; a socket path that moved \
                                    is a bug in how the window names it",
                    },
                );
                Link::default()
            }
        }
    }

    pub(crate) fn say(&self, report: &Report) {
        let Some(socket) = &self.socket else { return };
        let mut stream = poison::lock(socket, "bridge.link");
        // A window that has gone has nothing to tell; the bridge finds out from its stream.
        let _ = stream.write_all(report.line().as_bytes());
    }
}

/// Output counted between the reports that say it was painted.
///
/// Counted on the pump's thread and reported from one of its own, because the pump spends a
/// quiet pane blocked in a read, and the last writes of a burst are owed their report while it
/// is (kan a_2PeXwg4fA).
pub(crate) struct Counting {
    tallies: Mutex<Tallies>,
    arrived: Condvar,
    /// Whether anything was ever painted, which separates a pane that ended from one that
    /// never began.
    painted: AtomicBool,
}

struct Tallies {
    /// Repaint counts for the log, a line a second at most: the answer to "did the pane react
    /// to what I typed", which per-write records would bury.
    summary: Tally,
    /// The same counts for the window, on an interval of its own: it bounds how long a
    /// keystroke answered at the end of a burst can look unanswered.
    report: Tally,
}

const SUMMARY_INTERVAL_NS: u64 = 1_000_000_000;

impl Counting {
    pub(crate) fn new() -> Counting {
        Counting {
            tallies: Mutex::new(Tallies {
                summary: Tally::new(SUMMARY_INTERVAL_NS),
                report: Tally::new(PAINTED_INTERVAL_NS),
            }),
            arrived: Condvar::new(),
            painted: AtomicBool::new(false),
        }
    }

    pub(crate) fn painted(&self) -> bool {
        self.painted.load(Ordering::Relaxed)
    }

    pub(crate) fn count(&self, bytes: usize) {
        self.painted.store(true, Ordering::Relaxed);
        let mut tallies = poison::lock(&self.tallies, "bridge.tallies");
        let summary_was_quiet = tallies.summary.count(bytes);
        let report_was_quiet = tallies.report.count(bytes);
        drop(tallies);
        if summary_was_quiet || report_was_quiet {
            self.arrived.notify_one();
        }
    }

    /// Says what was counted, each when its interval ends, whether or not more has arrived by
    /// then. A pane painting nothing says nothing and costs this thread no wakeups.
    pub(crate) fn report(&self, link: &Link) -> ! {
        let mut tallies = poison::lock(&self.tallies, "bridge.tallies");
        loop {
            let now = monotonic_now();
            let summary = tallies.summary.take(now);
            let report = tallies.report.take(now);
            if summary.is_some() || report.is_some() {
                // Not under the lock: the pump takes it on every write, and a window that has
                // stopped reading would stall painting behind it.
                drop(tallies);
                if let Some(Counted { writes, bytes }) = summary {
                    log::debug("bridge.painted", fields! { "writes" => writes, "bytes" => bytes });
                }
                if let Some(Counted { writes, bytes }) = report {
                    link.say(&Report::Painted { writes, bytes });
                }
                tallies = poison::lock(&self.tallies, "bridge.tallies");
                continue;
            }
            let owed = tallies.report.due_in(now);
            tallies = match tallies.summary.due_in(now).into_iter().chain(owed).min() {
                Some(nanos) => {
                    let waited = self.arrived.wait_timeout(tallies, Duration::from_nanos(nanos));
                    poison::recover(waited, "bridge.tallies").0
                }
                None => poison::recover(self.arrived.wait(tallies), "bridge.tallies"),
            };
        }
    }
}

/// The surface, with every write counted on the way.
pub(crate) struct CountedSurface<'a, W: Write> {
    pub(crate) surface: W,
    pub(crate) counting: &'a Counting,
}

impl<W: Write> Write for CountedSurface<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.surface.write(bytes)?;
        self.counting.count(written);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.surface.flush()
    }
}
