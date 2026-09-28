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
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{
    self as proto, ConnectionKind, Welcome, answer, control_message, pane_request,
    request::Service, session_request,
};

/// What the reader thread hands the caller, in the order the daemon sent it.
#[derive(Debug, PartialEq)]
pub enum Delivered {
    /// Boxed because a pane's record makes an event far larger than the other variants.
    Event(Box<proto::Event>),
    /// The snapshot a subscribe was answered with, which every event delivered after it
    /// applies to. Delivered whether or not anyone still waits for the answer, so the events
    /// that follow always land on a picture the caller was given.
    Subscribed(Box<proto::Snapshot>),
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

impl std::fmt::Display for Unanswered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Unanswered::TimedOut => "no answer came in time",
            Unanswered::Ended => "the connection ended before an answer came",
        })
    }
}

/// A request sent, and the answer on its way.
#[derive(Debug)]
pub struct Pending {
    answer: Receiver<Arrived>,
    id: u64,
    request: &'static str,
    sent: Instant,
}

/// An answer, and when the reader took it off the socket.
type Arrived = (proto::Answer, Instant);

/// An answer later than this is logged at info, since it is somebody waiting.
const SLOW: Duration = Duration::from_secs(1);

impl Pending {
    /// Waits up to `patience` for the answer.
    ///
    /// Says how long it took, in two parts: until the reader took the answer off the socket,
    /// which is the daemon and everything queued ahead of the answer, and until this thread ran
    /// again to take it. On a loaded machine the second is the one a thread's priority decides.
    pub fn wait(&self, patience: Duration) -> Result<proto::Answer, Unanswered> {
        let ms = |elapsed: Duration| format!("{:.1}", elapsed.as_secs_f64() * 1000.0);
        match self.answer.recv_timeout(patience) {
            Ok((answer, read)) => {
                let took = self.sent.elapsed();
                let fields = fields! {
                    "id" => self.id,
                    "request" => self.request,
                    "ms" => ms(took),
                    "read_ms" => ms(read.duration_since(self.sent)),
                };
                if took > SLOW {
                    log::info("daemon.answer.slow", fields);
                } else {
                    log::debug("daemon.answered", fields);
                }
                Ok(answer)
            }
            Err(RecvTimeoutError::Timeout) => {
                log::warn(
                    "daemon.answer.late",
                    fields! {
                        "id" => self.id,
                        "request" => self.request,
                        "waited_ms" => ms(patience),
                        "impact" => "the caller is told the daemon did not answer; the request \
                                     may still take effect",
                        "check" => "the daemon's daemon.request.answered line for this id, \
                                    which says whether it answered late or the answer waited \
                                    here, and the machine's load",
                    },
                );
                Err(Unanswered::TimedOut)
            }
            Err(RecvTimeoutError::Disconnected) => Err(Unanswered::Ended),
        }
    }
}

/// An open control connection. Dropping it hangs up.
#[derive(Debug)]
pub struct Control {
    requests: Requests,
    welcome: Welcome,
    socket: UnixStream,
}

/// What `deliver` is handed to send requests with.
///
/// It sends and never offers an answer to wait for, because `deliver` runs on the one thread
/// that hands answers over: waiting there for an answer would wait on itself. A snapshot a
/// subscribe sent from here brings arrives through `deliver` as [`Delivered::Subscribed`].
#[derive(Debug, Clone)]
pub struct Requests {
    to_send: Sender<proto::Request>,
    waiting: Arc<Mutex<Waiting>>,
    next_id: Arc<AtomicU64>,
}

impl Requests {
    /// Subscribes again, as a [`Delivered::Gap`] asks.
    pub fn subscribe(&self) {
        self.send(subscribe(), true);
    }

    /// Follows the daemon's log from after record `after`.
    pub fn follow_log(&self, after: Option<u64>) {
        self.send(follow_log(after), false);
    }

    fn send(&self, service: Service, subscribes: bool) -> Pending {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (answer, answered) = mpsc::channel();
        let request = muster_daemon_proto::service_name(&service);
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            // Left out once the connection has ended: dropping the sender answers `Ended`.
            if !waiting.ended {
                waiting.answers.insert(id, Waiter { answer, subscribes });
            }
        }
        // Registered before it is sent, so the answer cannot arrive before its waiter.
        let _ = self.to_send.send(proto::Request { id, service: Some(service) });
        Pending { answer: answered, id, request, sent: Instant::now() }
    }
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
    answer: Sender<Arrived>,
    /// A subscribe, whose snapshot says which event comes next.
    subscribes: bool,
}

impl Control {
    /// Dials `socket` and opens a control connection. `client` says who is asking, for the
    /// daemon's log.
    ///
    /// `deliver` hears every event, snapshot and log line, on the connection's own thread, and
    /// an answer is handed to its waiter only once `deliver` has returned from the events before
    /// it. So `deliver` must never wait on a [`Pending`]: the answer could only come from the
    /// thread it is holding. What it needs to ask, such as subscribing again after a gap, it
    /// sends through the [`Requests`] it is given.
    pub fn open(
        socket: &Path,
        client: &str,
        deliver: impl FnMut(Delivered, &Requests) + Send + 'static,
    ) -> Result<Control, HandshakeError> {
        let (stream, welcome) = crate::dial(socket, ConnectionKind::Control, client)?;
        log::info(
            "daemon.control.opened",
            fields! {
                "socket" => socket.display(),
                "daemon_pid" => welcome.pid,
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
        deliver: impl FnMut(Delivered, &Requests) + Send + 'static,
    ) -> std::io::Result<Control> {
        let (sender, to_send) = mpsc::channel();
        let requests = Requests {
            to_send: sender,
            waiting: Arc::new(Mutex::new(Waiting::default())),
            next_id: Arc::new(AtomicU64::new(0)),
        };
        let writer_end = stream.try_clone()?;
        std::thread::Builder::new()
            .name("muster-daemon-control-writer".into())
            .spawn(move || write_requests(writer_end, &to_send))?;
        let reader_end = stream.try_clone()?;
        let for_reader = requests.clone();
        std::thread::Builder::new()
            .name("muster-daemon-control-reader".into())
            .spawn(move || read_messages(reader_end, &for_reader, deliver))?;
        Ok(Control { requests, welcome, socket: stream })
    }

    /// Who answered: the daemon's version, install, process and lifetime.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Sends a request. Never blocks.
    pub fn ask(&self, service: Service) -> Pending {
        self.requests.send(service, false)
    }

    /// The daemon's state now, and every event after it, delivered as it happens. The snapshot
    /// is delivered as [`Delivered::Subscribed`], and the answer carries it as well.
    pub fn subscribe(&self) -> Pending {
        self.requests.send(subscribe(), true)
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
        self.ask(follow_log(after))
    }

    /// Asks the daemon to hand every pane to `program`, started with `data` as its data
    /// directory, or with whatever it finds for itself when there is none (MIP-3, section 10).
    pub fn replace(&self, program: &Path, data: Option<&Path>) -> Pending {
        self.ask(session(session_request::Request::Replace(session_request::Replace {
            program: Some(program.display().to_string()),
            data: data.map(|data| data.display().to_string()),
        })))
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

    pub fn set_scroll_multiplier(&self, multiplier: f64) -> Pending {
        self.ask(session(session_request::Request::SetScrollMultiplier(
            proto::SetScrollMultiplier { multiplier },
        )))
    }

    /// Says a window with the keyboard is showing these panes, which clears each one's
    /// `finished_unseen`.
    pub fn seen(&self, panes: Vec<String>) -> Pending {
        self.ask(Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Seen(pane_request::Seen { panes })),
        }))
    }

    pub fn send_manifests(&self, engine: u32, manifests: Vec<proto::Manifest>) -> Pending {
        self.ask(session(session_request::Request::SendManifests(proto::SendManifests {
            engine,
            manifests,
        })))
    }
}

impl Control {
    /// Hangs up now, whoever else holds this connection: a request waiting on it is answered
    /// with its end rather than waiting out its patience.
    pub fn hang_up(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
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

fn subscribe() -> Service {
    session(session_request::Request::Subscribe(session_request::Subscribe { attends: false }))
}

fn follow_log(after: Option<u64>) -> Service {
    session(session_request::Request::FollowLog(session_request::FollowLog { after }))
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
    requests: &Requests,
    mut deliver: impl FnMut(Delivered, &Requests),
) {
    let waiting = &requests.waiting;
    let mut order = Order::Unsubscribed;
    // Answers that arrived while events were being dropped, handed over once a new snapshot
    // has been delivered: the events that carried their effects are in it, and not before.
    let mut held_back: Vec<(Waiter, Arrived)> = Vec::new();
    let why = loop {
        let message = match connection::receive::<proto::ControlMessage>(&mut stream) {
            Ok(Some(message)) => (message.message, Instant::now()),
            Ok(None) => break "the daemon hung up".to_string(),
            Err(error) => break format!("reading from the daemon failed: {error}"),
        };
        let (message, read) = message;
        match message {
            Some(control_message::Message::Event(event)) => match order {
                Order::Next(expected) if event.seq == expected => {
                    order = Order::Next(expected + 1);
                    deliver(Delivered::Event(Box::new(event)), requests);
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
                    deliver(Delivered::Gap { expected, got: event.seq }, requests);
                }
                // A subscribe's snapshot supersedes both: what the dropped events said is in it.
                Order::Lost | Order::Unsubscribed => {}
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
                    // The order moves only together with a snapshot the caller is given, so
                    // the events after it can never land on a picture that never had it.
                    order = Order::Next(snapshot.seq + 1);
                    deliver(Delivered::Subscribed(Box::new(snapshot.clone())), requests);
                    for (waiter, answer) in held_back.drain(..) {
                        let _ = waiter.answer.send(answer);
                    }
                } else if waiter.subscribes && matches!(order, Order::Lost) {
                    // Nothing else can bring the order back, so every event would be dropped and
                    // every held answer kept waiting until the socket happened to end. Ending it
                    // now has the follower reconnect in full.
                    let reason = answer.reason.clone();
                    let _ = waiter.answer.send((answer, read));
                    break format!(
                        "the daemon answered a subscribe after missed events without its state \
                         ({reason})"
                    );
                } else if matches!(order, Order::Lost) {
                    held_back.push((waiter, (answer, read)));
                    continue;
                }
                // A caller that stopped waiting has dropped its receiver.
                let _ = waiter.answer.send((answer, read));
            }
            Some(control_message::Message::LogLine(line)) => {
                deliver(Delivered::Log(line), requests);
            }
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
    deliver(Delivered::Ended(why), requests);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A control connection over a socket pair, and the daemon's end of it.
    fn connected() -> (Control, UnixStream, Receiver<Delivered>) {
        let (tell, delivered) = mpsc::channel();
        let (control, theirs) = connected_with(move |what, _| {
            let _ = tell.send(what);
        });
        (control, theirs, delivered)
    }

    fn connected_with(
        deliver: impl FnMut(Delivered, &Requests) + Send + 'static,
    ) -> (Control, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        (Control::over(ours, Welcome::default(), deliver).unwrap(), theirs)
    }

    /// What was delivered, in order, as short words: `S5` a snapshot at 5, `6` an event.
    fn described(what: &Delivered) -> String {
        match what {
            Delivered::Subscribed(snapshot) => format!("S{}", snapshot.seq),
            Delivered::Event(event) => event.seq.to_string(),
            Delivered::Gap { expected, got } => format!("gap {expected}->{got}"),
            other => panic!("{other:?}"),
        }
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

        let seqs: Vec<String> = delivered.iter().take(5).map(|what| described(&what)).collect();
        assert_eq!(seqs, ["S5", "6", "gap 7->8", "S9", "10"]);
    }

    /// A caller that stopped waiting for its subscribe still gets the snapshot, so the events
    /// after it are never applied to a picture that did not have it.
    #[test]
    fn a_subscribe_nobody_waits_for_still_delivers_its_snapshot_first() {
        let (control, mut daemon, delivered) = connected();
        drop(control.subscribe());
        subscribed_at(&mut daemon, 5);
        for message in [event(6, "a"), event(7, "b")] {
            connection::send(&mut daemon, &message).unwrap();
        }
        let seqs: Vec<String> = delivered.iter().take(3).map(|what| described(&what)).collect();
        assert_eq!(seqs, ["S5", "6", "7"]);
    }

    /// An answer that arrives while events are being dropped waits for the snapshot that holds
    /// them, so a request's effect is in the caller's picture by the time it returns.
    #[test]
    fn an_answer_during_a_gap_waits_for_the_next_snapshot() {
        let (control, mut daemon, delivered) = connected();
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 5);
        subscribed.wait(PATIENCE).unwrap();
        for message in [event(6, "a"), event(8, "b")] {
            connection::send(&mut daemon, &message).unwrap();
        }

        let asked = control.snapshot();
        answer_to(&mut daemon, None);
        assert!(
            asked.wait(Duration::from_millis(200)).is_err(),
            "answered while the events behind the answer were being dropped"
        );
        let resubscribed = control.subscribe();
        subscribed_at(&mut daemon, 9);
        resubscribed.wait(PATIENCE).unwrap();
        asked.wait(PATIENCE).expect("answered once the snapshot is delivered");

        let seqs: Vec<String> = delivered.iter().take(4).map(|what| described(&what)).collect();
        assert_eq!(seqs, ["S5", "6", "gap 7->8", "S9"]);
    }

    /// A subscribe after a gap that is answered without a snapshot ends the connection at once,
    /// since nothing else could bring the order back: every event after it would be dropped,
    /// and every answer held until the socket happened to end.
    #[test]
    fn a_resubscribe_refused_during_a_gap_ends_the_connection() {
        let (control, mut daemon, delivered) = connected();
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 5);
        subscribed.wait(PATIENCE).unwrap();
        connection::send(&mut daemon, &event(7, "a")).unwrap();

        let resubscribed = control.subscribe();
        answer_to(&mut daemon, None);
        resubscribed.wait(PATIENCE).expect("the refusal is still an answer");
        let ended = std::iter::from_fn(|| delivered.recv_timeout(PATIENCE).ok())
            .find_map(|what| match what {
                Delivered::Ended(why) => Some(why),
                _ => None,
            })
            .expect("the connection ends within the patience");
        assert!(ended.contains("without its state"), "ended saying {ended:?}");
    }

    /// Nothing is delivered before the first snapshot: it is the whole picture, and an event
    /// ahead of it would be applied to none.
    #[test]
    fn an_event_before_the_first_snapshot_is_not_delivered() {
        let (control, mut daemon, delivered) = connected();
        connection::send(&mut daemon, &event(3, "early")).unwrap();
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 5);
        subscribed.wait(PATIENCE).unwrap();
        connection::send(&mut daemon, &event(6, "a")).unwrap();
        let seqs: Vec<String> = delivered.iter().take(2).map(|what| described(&what)).collect();
        assert_eq!(seqs, ["S5", "6"]);
    }

    /// A gap is answered from inside `deliver`, which cannot wait, and the order recovers.
    #[test]
    fn a_gap_is_recovered_by_a_subscribe_sent_from_deliver() {
        let (tell, delivered) = mpsc::channel();
        let (control, mut daemon) = connected_with(move |what, requests| {
            if matches!(what, Delivered::Gap { .. }) {
                requests.subscribe();
            }
            let _ = tell.send(what);
        });
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 5);
        subscribed.wait(PATIENCE).unwrap();
        for message in [event(6, "a"), event(8, "b")] {
            connection::send(&mut daemon, &message).unwrap();
        }
        subscribed_at(&mut daemon, 8);
        connection::send(&mut daemon, &event(9, "c")).unwrap();
        let seqs: Vec<String> = delivered.iter().take(5).map(|what| described(&what)).collect();
        assert_eq!(seqs, ["S5", "6", "gap 7->8", "S8", "9"]);
    }

    /// An answer waits for `deliver` to finish with the events before it, so a caller that
    /// applies events in `deliver` sees its request take effect before the answer.
    #[test]
    fn a_waiter_is_answered_only_once_the_events_before_its_answer_are_delivered() {
        let (release, released) = mpsc::channel::<()>();
        let (control, mut daemon) = connected_with(move |what, _| {
            if matches!(what, Delivered::Event(_)) {
                let _ = released.recv();
            }
        });
        let subscribed = control.subscribe();
        subscribed_at(&mut daemon, 0);
        subscribed.wait(PATIENCE).unwrap();

        let pending = control.snapshot();
        connection::send(&mut daemon, &event(1, "a")).unwrap();
        answer_to(&mut daemon, None);
        assert_eq!(
            pending.wait(Duration::from_millis(200)).unwrap_err(),
            Unanswered::TimedOut,
            "answered while the event before it was still being delivered"
        );
        release.send(()).unwrap();
        pending.wait(PATIENCE).unwrap();
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
