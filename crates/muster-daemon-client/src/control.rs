//! A control connection: requests and their answers, the daemon's events in order, and its log.
//!
//! One per app per daemon (MIP-3, section 9). Asking never blocks the caller, since the core's
//! writes to a daemon never block its main thread: a writer thread sends what is asked, and
//! waiting for the answer is the caller's choice. A reader thread hands every event and log line
//! to the caller in the order the daemon sent them, and an answer to its waiter only after the
//! events it produced, so a caller that applies events and waits for an answer sees its request
//! take effect before the answer arrives.

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{
    self as proto, ConnectionKind, Welcome, answer, control_message, request::Service,
    session_request,
};

/// What the reader thread hands the caller, in the order the daemon sent it.
#[derive(Debug, PartialEq)]
pub enum Delivered {
    /// Boxed because a pane's record makes an event far larger than the other variants.
    Event(Box<proto::Event>),
    /// An event arrived out of order, so the caller's picture is missing something. Nothing
    /// more is delivered until the caller subscribes again, and the snapshot that answers says
    /// where things stand.
    Gap { expected: u64, got: u64 },
    /// One record of the daemon's own log, on a connection that asked to follow it.
    Log(proto::LogLine),
    /// The connection is over, for the reason given. Delivered once, including when this side
    /// ended it by dropping its [`Control`].
    Ended(String),
}

/// Why a request has no answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Unanswered {
    /// None came in the time the caller gave.
    TimedOut,
    /// The connection ended first. The daemon may or may not have done what was asked.
    Ended,
}

/// A request sent, and the answer on its way.
#[derive(Debug)]
pub struct Pending {
    answer: Receiver<proto::Answer>,
}

impl Pending {
    /// Waits up to `patience` for the answer.
    pub fn wait(&self, patience: Duration) -> Result<proto::Answer, Unanswered> {
        self.answer.recv_timeout(patience).map_err(|error| match error {
            RecvTimeoutError::Timeout => Unanswered::TimedOut,
            RecvTimeoutError::Disconnected => Unanswered::Ended,
        })
    }
}

/// An open control connection. Dropping it hangs up.
#[derive(Debug)]
pub struct Control {
    requests: Sender<proto::Request>,
    waiting: Arc<Mutex<Waiting>>,
    next_id: AtomicU64,
    welcome: Welcome,
    socket: UnixStream,
}

/// Requests sent and not yet answered.
#[derive(Debug, Default)]
struct Waiting {
    answers: HashMap<u64, Waiter>,
    /// Once the connection has ended no answer can come, so a request asked afterwards is
    /// answered `Ended` at once rather than left to time out.
    ended: bool,
}

#[derive(Debug)]
struct Waiter {
    answer: Sender<proto::Answer>,
    /// A subscribe, whose snapshot says which event comes next.
    subscribes: bool,
}

impl Control {
    /// Dials `socket` and opens a control connection. `client` says who is asking, for the
    /// daemon's log; `deliver` hears every event and log line, on the connection's own thread.
    pub fn open(
        socket: &Path,
        client: &str,
        deliver: impl FnMut(Delivered) + Send + 'static,
    ) -> Result<Control, HandshakeError> {
        let (stream, welcome) = crate::dial(socket, ConnectionKind::Control, client)?;
        log::info(
            "daemon.control.opened",
            fields! {
                "socket" => socket.display(),
                "pid" => welcome.pid,
                "instance" => welcome.instance,
                "version" => welcome.daemon_version,
                "install" => welcome.install,
            },
        );
        Control::over(stream, welcome, deliver)
            .map_err(|error| HandshakeError::Unreachable(error.to_string()))
    }

    /// The same, on a connection whose handshake is done.
    fn over(
        stream: UnixStream,
        welcome: Welcome,
        deliver: impl FnMut(Delivered) + Send + 'static,
    ) -> std::io::Result<Control> {
        let waiting = Arc::new(Mutex::new(Waiting::default()));
        let (requests, to_send) = mpsc::channel();
        let writer_end = stream.try_clone()?;
        std::thread::Builder::new()
            .name("muster-daemon-control-writer".into())
            .spawn(move || write_requests(writer_end, &to_send))?;
        let reader_end = stream.try_clone()?;
        let answers = Arc::clone(&waiting);
        std::thread::Builder::new()
            .name("muster-daemon-control-reader".into())
            .spawn(move || read_messages(reader_end, &answers, deliver))?;
        Ok(Control { requests, waiting, next_id: AtomicU64::new(0), welcome, socket: stream })
    }

    /// Who answered: the daemon's version, install, process and lifetime.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Sends a request. Never blocks.
    pub fn ask(&self, service: Service) -> Pending {
        self.send(service, false)
    }

    /// The daemon's state now, and every event after it, delivered as it happens. The answer
    /// carries the snapshot.
    pub fn subscribe(&self) -> Pending {
        self.send(session(session_request::Request::Subscribe(session_request::Subscribe {})), true)
    }

    /// The daemon's state now, without subscribing.
    pub fn snapshot(&self) -> Pending {
        self.ask(session(session_request::Request::Snapshot(session_request::Snapshot {})))
    }

    /// Every pane closed and the daemon gone. Answered before it goes.
    pub fn stop(&self) -> Pending {
        self.ask(session(session_request::Request::Stop(session_request::Stop {})))
    }

    /// The daemon's log from after record `after` of this daemon run, or all it still holds,
    /// and every record after that, delivered as [`Delivered::Log`].
    pub fn follow_log(&self, after: Option<u64>) -> Pending {
        self.ask(session(session_request::Request::FollowLog(session_request::FollowLog { after })))
    }

    pub fn set_palette(&self, palette: proto::Palette) -> Pending {
        self.ask(session(session_request::Request::SetPalette(proto::SetPalette {
            palette: Some(palette),
        })))
    }

    pub fn set_cursor(&self, cursor: proto::Cursor) -> Pending {
        self.ask(session(session_request::Request::SetCursor(proto::SetCursor {
            cursor: Some(cursor),
        })))
    }

    pub fn set_shell(&self, shell: proto::Shell) -> Pending {
        self.ask(session(session_request::Request::SetShell(proto::SetShell {
            shell: Some(shell),
        })))
    }

    pub fn set_scrollback(&self, bytes: Option<u64>) -> Pending {
        self.ask(session(session_request::Request::SetScrollback(proto::SetScrollback { bytes })))
    }

    pub fn set_clipboard_write(&self, allowed: bool) -> Pending {
        self.ask(session(session_request::Request::SetClipboardWrite(proto::SetClipboardWrite {
            allowed,
        })))
    }

    pub fn send_manifests(&self, engine: u32, manifests: Vec<proto::Manifest>) -> Pending {
        self.ask(session(session_request::Request::SendManifests(proto::SendManifests {
            engine,
            manifests,
        })))
    }

    fn send(&self, service: Service, subscribes: bool) -> Pending {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (answer, answered) = mpsc::channel();
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            // Left out once the connection has ended: dropping the sender answers `Ended`.
            if !waiting.ended {
                waiting.answers.insert(id, Waiter { answer, subscribes });
            }
        }
        // Registered before it is sent, so the answer cannot arrive before its waiter.
        let _ = self.requests.send(proto::Request { id, service: Some(service) });
        Pending { answer: answered }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        // Both threads hold their own handles on the socket; shutting it down ends them.
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

fn session(request: session_request::Request) -> Service {
    Service::Session(proto::SessionRequest { request: Some(request) })
}

fn write_requests(mut stream: UnixStream, requests: &Receiver<proto::Request>) {
    for request in requests {
        if let Err(error) = connection::send(&mut stream, &request) {
            // The reader sees the same failure and reports the connection's end.
            log::debug("daemon.control.write_failed", fields! { "error" => error });
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Where the event sequence stands, which is known only once a subscribe is answered.
enum Order {
    Unsubscribed,
    Next(u64),
    /// A gap was delivered; everything waits for the snapshot of a new subscribe.
    Lost,
}

fn read_messages(
    mut stream: UnixStream,
    waiting: &Mutex<Waiting>,
    mut deliver: impl FnMut(Delivered),
) {
    let mut order = Order::Unsubscribed;
    let why = loop {
        let message = match connection::receive::<proto::ControlMessage>(&mut stream) {
            Ok(Some(message)) => message.message,
            Ok(None) => break "the daemon hung up".to_string(),
            Err(error) => break format!("reading from the daemon failed: {error}"),
        };
        match message {
            Some(control_message::Message::Event(event)) => match order {
                Order::Next(expected) if event.seq == expected => {
                    order = Order::Next(expected + 1);
                    deliver(Delivered::Event(Box::new(event)));
                }
                // Already in the snapshot a new subscribe answered with.
                Order::Next(expected) if event.seq < expected => {}
                Order::Next(expected) => {
                    log::warn(
                        "daemon.control.gap",
                        fields! {
                            "expected" => expected,
                            "got" => event.seq,
                            "impact" => "events were missed, so what the window shows may be \
                                         out of date until it subscribes again",
                            "check" => "the daemon's log for a subscriber it dropped; this is \
                                        likely a bug, since a daemon never skips an event",
                        },
                    );
                    order = Order::Lost;
                    deliver(Delivered::Gap { expected, got: event.seq });
                }
                Order::Lost => {}
                Order::Unsubscribed => deliver(Delivered::Event(Box::new(event))),
            },
            Some(control_message::Message::Answer(answer)) => {
                let waiter = waiting
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .answers
                    .remove(&answer.id);
                let Some(waiter) = waiter else {
                    log::warn(
                        "daemon.control.unasked",
                        fields! {
                            "id" => answer.id,
                            "impact" => "none: an answer nobody is waiting for is dropped",
                            "check" => "the daemon's log; this is likely a bug in the daemon",
                        },
                    );
                    continue;
                };
                if waiter.subscribes
                    && let Some(answer::Detail::Snapshot(snapshot)) = &answer.detail
                {
                    order = Order::Next(snapshot.seq + 1);
                }
                // A caller that stopped waiting has dropped its receiver.
                let _ = waiter.answer.send(answer);
            }
            Some(control_message::Message::LogLine(line)) => deliver(Delivered::Log(line)),
            None => {}
        }
    };
    {
        let mut waiting = waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.ended = true;
        // Dropping every waiter's sender answers each of them `Ended`.
        waiting.answers.clear();
    }
    log::info("daemon.control.ended", fields! { "why" => why });
    deliver(Delivered::Ended(why));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A control connection over a socket pair, and the daemon's end of it.
    fn connected() -> (Control, UnixStream, Receiver<Delivered>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (tell, delivered) = mpsc::channel();
        let control = Control::over(ours, Welcome::default(), move |what| {
            let _ = tell.send(what);
        })
        .unwrap();
        (control, theirs, delivered)
    }

    fn event(seq: u64, tab: &str) -> proto::ControlMessage {
        proto::ControlMessage {
            message: Some(control_message::Message::Event(proto::Event {
                seq,
                event: Some(proto::event::Event::TabClosed(proto::TabClosed { tab: tab.into() })),
            })),
        }
    }

    fn answer_to(daemon: &mut UnixStream, detail: Option<answer::Detail>) {
        let request: proto::Request = connection::receive(daemon).unwrap().unwrap();
        let answer = proto::Answer {
            id: request.id,
            outcome: proto::Outcome::Done.into(),
            detail,
            ..proto::Answer::default()
        };
        let message =
            proto::ControlMessage { message: Some(control_message::Message::Answer(answer)) };
        connection::send(daemon, &message).unwrap();
    }

    fn subscribed_at(daemon: &mut UnixStream, seq: u64) {
        let snapshot = proto::Snapshot { seq, ..proto::Snapshot::default() };
        answer_to(daemon, Some(answer::Detail::Snapshot(snapshot)));
    }

    const PATIENCE: Duration = Duration::from_secs(5);

    #[test]
    fn an_event_out_of_order_is_a_gap_and_nothing_more_arrives_until_a_new_subscribe() {
        let (control, mut daemon, delivered) = connected();
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 5);
        subscribed.wait(PATIENCE).unwrap();
        for message in [event(6, "a"), event(8, "b"), event(9, "c")] {
            connection::send(&mut daemon, &message).unwrap();
        }

        let resubscribed = control.subscribe();
        subscribed_at(&mut daemon, 9);
        resubscribed.wait(PATIENCE).unwrap();
        for message in [event(9, "old"), event(10, "d")] {
            connection::send(&mut daemon, &message).unwrap();
        }

        let seqs: Vec<String> = delivered
            .iter()
            .take(3)
            .map(|what| match what {
                Delivered::Event(event) => event.seq.to_string(),
                Delivered::Gap { expected, got } => format!("gap {expected}->{got}"),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(seqs, ["6", "gap 7->8", "10"]);
    }

    #[test]
    fn a_waiter_is_answered_after_the_events_before_its_answer() {
        let (control, mut daemon, delivered) = connected();
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 0);
        subscribed.wait(PATIENCE).unwrap();

        let pending =
            control.ask(session(session_request::Request::Snapshot(session_request::Snapshot {})));
        connection::send(&mut daemon, &event(1, "a")).unwrap();
        answer_to(&mut daemon, None);
        pending.wait(PATIENCE).unwrap();
        assert!(
            matches!(delivered.try_recv(), Ok(Delivered::Event(_))),
            "the event was delivered by the time its answer was"
        );
    }

    #[test]
    fn a_connection_that_ends_answers_every_waiter_and_says_so_once() {
        let (control, daemon, delivered) = connected();
        let pending = control.snapshot();
        drop(daemon);
        assert_eq!(pending.wait(PATIENCE).unwrap_err(), Unanswered::Ended);
        assert!(matches!(delivered.recv_timeout(PATIENCE), Ok(Delivered::Ended(_))));
        assert_eq!(
            control.snapshot().wait(PATIENCE).unwrap_err(),
            Unanswered::Ended,
            "a request after the end is answered at once"
        );
        assert!(delivered.recv_timeout(Duration::from_millis(50)).is_err(), "said once");
    }
}
