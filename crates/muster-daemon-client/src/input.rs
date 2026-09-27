//! The input connection: keystrokes, clicks, pastes and sends for every pane on one daemon.
//!
//! Never answered, and never allowed to hold up its caller (MIP-3, section 9). Events queue for
//! a writer thread, and when the queue is full an event is dropped rather than waited for: the
//! daemon drains this connection into each pane's own queue, so a full one here means the daemon
//! itself is stalled, and a window that froze with it would be worse than lost keystrokes.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{self as proto, ConnectionKind};

/// Events waiting for the writer. Far more than a person types or a program sends between two
/// writes, so a full queue means the daemon stopped reading.
const QUEUE: usize = 1024;

/// An open input connection. Dropping it hangs up once what is queued has been written.
#[derive(Debug)]
pub struct Input {
    events: SyncSender<proto::InputEvent>,
    /// Set while events are being dropped, so a stall is reported once rather than per event.
    dropping: AtomicBool,
    /// Cleared by the writer when a write fails.
    open: Arc<AtomicBool>,
}

impl Input {
    /// Dials `socket` and opens an input connection. `client` says who is asking.
    pub fn open(socket: &Path, client: &str) -> Result<Input, HandshakeError> {
        let (stream, _) = crate::dial(socket, ConnectionKind::Input, client)?;
        Input::over(stream).map_err(|error| HandshakeError::Unreachable(error.to_string()))
    }

    fn over(stream: UnixStream) -> std::io::Result<Input> {
        let (events, queued) = mpsc::sync_channel(QUEUE);
        let open = Arc::new(AtomicBool::new(true));
        let writer_open = Arc::clone(&open);
        std::thread::Builder::new()
            .name("muster-daemon-input-writer".into())
            .spawn(move || write_events(stream, &queued, &writer_open))?;
        Ok(Input { events, dropping: AtomicBool::new(false), open })
    }

    /// Queues `event` for the daemon. Never blocks: an event that finds the queue full, or the
    /// connection gone, is dropped.
    pub fn send(&self, event: proto::InputEvent) {
        match self.events.try_send(event) {
            Ok(()) => self.dropping.store(false, Ordering::Relaxed),
            Err(TrySendError::Full(event)) => {
                if !self.dropping.swap(true, Ordering::Relaxed) {
                    log::warn(
                        "daemon.input.dropped",
                        fields! {
                            "pane" => event.pane,
                            "queued" => QUEUE,
                            "impact" => "keystrokes and pastes are being lost until the daemon \
                                         reads its input again; this is said once per stall",
                            "check" => "whether the daemon is alive and responsive (its log, \
                                        and a snapshot); on a remote machine, whether the ssh \
                                        connection is stalled",
                        },
                    );
                }
            }
            // The writer said why when it stopped; the caller learns of it from the control
            // connection, which ends too.
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Whether events still reach the daemon. False once a write has failed, after which the
    /// caller opens a new connection.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

fn write_events(mut stream: UnixStream, events: &Receiver<proto::InputEvent>, open: &AtomicBool) {
    for event in events {
        if let Err(error) = connection::send(&mut stream, &event) {
            open.store(false, Ordering::Relaxed);
            log::warn(
                "daemon.input.ended",
                fields! {
                    "error" => error,
                    "impact" => "input reaches no pane on this daemon until it reconnects",
                    "check" => "whether the daemon is still running; its control connection \
                                ends too, and says why",
                },
            );
            return;
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
        let input = Input::over(ours).unwrap();
        let text = "x".repeat(1024);
        let started = Instant::now();
        for _ in 0..20_000 {
            input.send(typed(&text));
        }
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        assert!(input.dropping.load(Ordering::Relaxed), "what did not fit was dropped");
        assert!(input.is_open(), "a stalled daemon is not a closed one");
    }

    #[test]
    fn a_daemon_that_hung_up_closes_the_connection() {
        let (ours, daemon) = UnixStream::pair().unwrap();
        let input = Input::over(ours).unwrap();
        drop(daemon);
        let deadline = Instant::now() + Duration::from_secs(5);
        while input.is_open() && Instant::now() < deadline {
            input.send(typed("x"));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!input.is_open());
    }
}
