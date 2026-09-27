//! The input connection: keystrokes, clicks, pastes and sends for every pane on one daemon.
//!
//! Never answered, and never allowed to hold up its caller (MIP-3, section 9). Events queue for
//! a writer thread, and when the queue is full an event is dropped rather than waited for: the
//! daemon drains this connection into each pane's own queue, so a full one here means the daemon
//! itself is stalled, and a window that froze with it would be worse than lost keystrokes.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::NotSent;
use muster_daemon_proto::connection::{self, HandshakeError, LARGEST_MESSAGE};
use muster_daemon_proto::{self as proto, ConnectionKind};
use prost::Message;

/// Events waiting for the writer. Far more than a person types or a program sends between two
/// writes, so a full queue means the daemon stopped reading.
const QUEUE: usize = 1024;

/// Bytes waiting for the writer, since a thousand queued pastes could otherwise hold a great
/// deal of memory. An event that finds the queue empty is taken whatever its size, so this
/// bounds what piles up behind a stalled daemon, never how large one paste may be.
const QUEUED_BYTES: usize = 4 << 20;

/// Called once, from the writer's thread, when a write fails on a connection nobody closed:
/// the daemon's rule is that a client whose input it could not read opens a new one.
pub type OnClosed = Box<dyn FnOnce() + Send>;

/// An open input connection. Dropping it hangs up at once, dropping whatever is still queued.
#[derive(Debug)]
pub struct Input {
    events: SyncSender<proto::InputEvent>,
    /// Set while events are being dropped, so a stall is reported once rather than per event.
    dropping: AtomicBool,
    /// Cleared by the writer when a write fails.
    open: Arc<AtomicBool>,
    /// The encoded size of what is queued, which the writer lowers as it writes.
    queued: Arc<AtomicUsize>,
    /// Shut down on drop, so a writer blocked on a daemon that stopped reading ends too.
    socket: UnixStream,
}

impl Input {
    /// Dials `socket` and opens an input connection. `client` says who is asking.
    pub fn open(socket: &Path, client: &str, on_closed: OnClosed) -> Result<Input, HandshakeError> {
        let (stream, _) = crate::dial(socket, ConnectionKind::Input, client)?;
        Input::over(stream, on_closed)
            .map_err(|error| HandshakeError::Unreachable(error.to_string()))
    }

    fn over(stream: UnixStream, on_closed: OnClosed) -> std::io::Result<Input> {
        let (events, waiting) = mpsc::sync_channel(QUEUE);
        let open = Arc::new(AtomicBool::new(true));
        let queued = Arc::new(AtomicUsize::new(0));
        let socket = stream.try_clone()?;
        let writer = Writer { open: Arc::clone(&open), queued: Arc::clone(&queued), on_closed };
        std::thread::Builder::new()
            .name("muster-daemon-input-writer".into())
            .spawn(move || writer.write(stream, &waiting))?;
        Ok(Input { events, dropping: AtomicBool::new(false), open, queued, socket })
    }

    /// Queues `event` for the daemon. Never blocks: an event that finds the queue full, or the
    /// connection gone, is dropped, and so is one the daemon could not read in one message.
    pub fn send(&self, event: proto::InputEvent) -> Result<(), NotSent> {
        let size = event.encoded_len();
        let limit = LARGEST_MESSAGE as usize;
        if size > limit {
            return Err(NotSent::TooLarge { bytes: size, limit });
        }
        let before = self.queued.fetch_add(size, Ordering::Relaxed);
        let sent = if before > 0 && before + size > QUEUED_BYTES {
            Err(TrySendError::Full(event))
        } else {
            self.events.try_send(event)
        };
        if sent.is_err() {
            self.queued.fetch_sub(size, Ordering::Relaxed);
        }
        match sent {
            Ok(()) => {
                self.dropping.store(false, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Full(event)) => {
                if !self.dropping.swap(true, Ordering::Relaxed) {
                    log::warn(
                        "daemon.input.dropped",
                        fields! {
                            "pane" => event.pane,
                            "queued" => QUEUE,
                            "queued_bytes" => before,
                            "impact" => "keystrokes and pastes are being lost until the daemon \
                                         reads its input again; this is said once per stall",
                            "check" => "whether the daemon is alive and responsive (its log, \
                                        and a snapshot); on a remote machine, whether the ssh \
                                        connection is stalled",
                        },
                    );
                }
                Err(NotSent::Full)
            }
            // The writer said why when it stopped, and the follower is opening another.
            Err(TrySendError::Disconnected(_)) => Err(NotSent::NotConnected),
        }
    }

    /// Whether events still reach the daemon. False once a write has failed, after which the
    /// caller opens a new connection.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
impl Input {
    /// Ends the connection underneath the writer, as a daemon that could not read it does.
    pub(crate) fn break_underneath(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.open.store(false, Ordering::Relaxed);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

struct Writer {
    open: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    on_closed: OnClosed,
}

impl Writer {
    fn write(self, mut stream: UnixStream, events: &Receiver<proto::InputEvent>) {
        for event in events {
            let size = event.encoded_len();
            let written = connection::send(&mut stream, &event);
            self.queued.fetch_sub(size, Ordering::Relaxed);
            if let Err(error) = written {
                // Already closed means the caller hung up, which is no failure to report.
                if !self.open.swap(false, Ordering::Relaxed) {
                    return;
                }
                log::warn(
                    "daemon.input.ended",
                    fields! {
                        "error" => error,
                        "impact" => "what was queued for this daemon's panes is lost; input \
                                     resumes once a new input connection opens",
                        "check" => "the daemon's log for why it closed the connection; if it \
                                    is not running, its control connection ends too, and says why",
                    },
                );
                (self.on_closed)();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn typed(text: &str) -> proto::InputEvent {
        proto::InputEvent {
            pane: "p1".into(),
            input: Some(proto::input_event::Input::Send(proto::input_event::Send {
                text: text.into(),
                enter: false,
            })),
        }
    }

    /// A daemon that stopped reading fills the socket and then the queue, and the caller goes
    /// on at full speed, losing what does not fit.
    #[test]
    fn a_daemon_that_stops_reading_never_holds_the_caller() {
        let (ours, _daemon) = UnixStream::pair().unwrap();
        let input = Input::over(ours, Box::new(|| {})).unwrap();
        let text = "x".repeat(1024);
        let started = Instant::now();
        for _ in 0..20_000 {
            let _ = input.send(typed(&text));
        }
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        assert!(input.dropping.load(Ordering::Relaxed), "what did not fit was dropped");
        assert!(input.is_open(), "a stalled daemon is not a closed one");
    }

    /// Dropping the connection while the daemon has stopped reading ends the writer blocked on
    /// it, rather than leaving a thread and a descriptor behind for the life of the app.
    #[test]
    fn dropping_it_ends_a_writer_stuck_on_a_stalled_daemon() {
        let (ours, _daemon) = UnixStream::pair().unwrap();
        let input = Input::over(ours, Box::new(|| {})).unwrap();
        let text = "x".repeat(64 * 1024);
        for _ in 0..16 {
            let _ = input.send(typed(&text));
        }
        let queued = Arc::clone(&input.queued);
        drop(input);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&queued) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(Arc::strong_count(&queued), 1, "the writer thread is gone");
    }

    /// A few large pastes behind a stalled daemon are bounded by their size, not only by their
    /// number, while one paste larger than the bound still goes when nothing is queued.
    #[test]
    fn pastes_piling_up_are_bounded_by_their_size() {
        let (ours, _daemon) = UnixStream::pair().unwrap();
        let input = Input::over(ours, Box::new(|| {})).unwrap();
        let paste = "x".repeat(3 << 20);
        assert_eq!(input.send(typed(&paste)), Ok(()), "the first goes, whatever its size");
        let _ = input.send(typed(&paste));
        assert_eq!(input.send(typed(&paste)), Err(NotSent::Full), "past the bound is dropped");
        assert!(input.queued.load(Ordering::Relaxed) <= 2 * (3 << 20) + 64);
    }

    /// The caller hears that the connection closed, which is its cue to open another.
    #[test]
    fn a_daemon_that_hung_up_closes_the_connection_and_says_so() {
        let (ours, daemon) = UnixStream::pair().unwrap();
        let (closed, heard) = mpsc::channel();
        let input = Input::over(ours, Box::new(move || closed.send(()).unwrap())).unwrap();
        drop(daemon);
        let deadline = Instant::now() + Duration::from_secs(5);
        while input.is_open() && Instant::now() < deadline {
            let _ = input.send(typed("x"));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!input.is_open());
        heard.recv_timeout(Duration::from_secs(5)).expect("the close was reported");
    }

    /// Hanging up on purpose is not the daemon closing the connection, so nobody reopens it.
    #[test]
    fn dropping_it_is_not_reported_as_closed() {
        let (ours, _daemon) = UnixStream::pair().unwrap();
        let (closed, heard) = mpsc::channel();
        let input = Input::over(ours, Box::new(move || closed.send(()).unwrap())).unwrap();
        let _ = input.send(typed("x"));
        drop(input);
        assert!(heard.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// One message larger than the daemon reads would close the connection every pane on the
    /// daemon shares, so it is refused here and nothing of it is written.
    #[test]
    fn an_event_larger_than_one_message_is_refused() {
        let (ours, _daemon) = UnixStream::pair().unwrap();
        let input = Input::over(ours, Box::new(|| {})).unwrap();
        let limit = LARGEST_MESSAGE as usize;
        let refused = input.send(typed(&"x".repeat(limit)));
        assert!(matches!(refused, Err(NotSent::TooLarge { limit: l, .. }) if l == limit));
        assert_eq!(input.queued.load(Ordering::Relaxed), 0);
        assert!(input.is_open());
    }
}
