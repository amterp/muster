//! A control connection: requests in, answers and events out.
//!
//! Everything a connection sends goes through its [`Outbox`], a bounded queue drained by a
//! writer thread of its own. Queuing never blocks, so the session lock is never held waiting on
//! a slow client's socket; a client too slow to keep a queue this deep is dropped instead, and
//! subscribes again when it reconnects.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, pane_request, request::Service, session_request};
use prost::Message;

use crate::attribution;
use crate::restart::Restarting;
use crate::session::{Handled, Replacement, Reply, Session, Shared, Stop};
use crate::{handoff, messages};

/// How many messages a control connection may have waiting before the daemon gives up on it.
///
/// A client reading at all keeps this near empty: events are a few hundred bytes and arrive
/// at the rate panes change. Filling it means the client has stopped reading, and holding more
/// for it would only grow the daemon.
const QUEUE_DEPTH: usize = 4096;

/// How long a `stop` waits for its answer to be written before the daemon exits anyway.
const STOP_FLUSH: Duration = Duration::from_secs(2);
/// How long a report waits for a starting daemon's manifests before it is attributed to
/// nothing. They take tens of milliseconds to compile in a release build, and longer in a debug
/// build on a loaded machine. Half of `muster-daemon report`'s own patience, which covers dialing
/// and the answer too: a wait as long as the reporter's would time the reporter out instead.
const MANIFESTS_PATIENCE: Duration = Duration::from_secs(1);

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// How much a connection may have waiting before the daemon gives up on it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    pub(crate) messages: usize,
    pub(crate) bytes: usize,
}

impl Budget {
    /// A control connection's, which counts messages alone: what it carries is small and the
    /// app reads it.
    pub(crate) const CONTROL: Budget = Budget { messages: QUEUE_DEPTH, bytes: usize::MAX };
}

impl std::fmt::Display for Budget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.bytes == usize::MAX {
            write!(formatter, "{} messages", self.messages)
        } else {
            write!(formatter, "{} messages or {} bytes", self.messages, self.bytes)
        }
    }
}

/// Where a frame stands in its connection's numbered stream: the subscription's epoch, and the
/// number of the last event sent once the frame is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Position {
    pub(crate) epoch: u64,
    pub(crate) number: u64,
}

/// What a connection that fell behind is told before it is hung up: a frame built from the last
/// position written, written after the frame the writer was on.
pub(crate) struct Farewell {
    pub(crate) words: Box<dyn Fn(Position) -> Vec<u8> + Send>,
    /// How long the client has to read again before it is hung up on without them.
    pub(crate) patience: Duration,
}

/// What a connection has to send, in order.
#[derive(Debug, Clone)]
pub(crate) struct Outbox {
    pub(crate) id: u64,
    sender: SyncSender<Outbound>,
    stream: Arc<UnixStream>,
    budget: Budget,
    queued: Arc<Queued>,
}

/// What the pushing threads and the writer share about a connection's queue.
#[derive(Debug)]
struct Queued {
    bytes: AtomicUsize,
    /// The connection fell behind: nothing more is queued, and its writer stops at the next
    /// frame.
    overrun: AtomicBool,
    /// Disconnected once the writer has ended, which a farewell's patience waits for.
    writer_ended: Mutex<Option<mpsc::Receiver<()>>>,
    patience: Option<Duration>,
}

#[derive(Debug)]
enum Outbound {
    Frame(Arc<[u8]>, Option<Position>),
    /// A request's answer, and how it came about, logged once it is written.
    Answer(Arc<[u8]>, Answered),
    /// Answered once everything queued before it has been written.
    Flushed(mpsc::Sender<()>),
    /// Wakes a writer waiting for frames, to say its connection fell behind.
    Overrun,
}

impl Outbox {
    /// A control connection's outbox: [`Budget::CONTROL`], hung up on the moment it falls
    /// behind.
    pub(crate) fn open(stream: &UnixStream) -> std::io::Result<Outbox> {
        Outbox::open_with(stream, Budget::CONTROL, None)
    }

    pub(crate) fn open_with(
        stream: &UnixStream,
        budget: Budget,
        farewell: Option<Farewell>,
    ) -> std::io::Result<Outbox> {
        let id = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(budget.messages);
        let (ending, writer_ended) = mpsc::channel::<()>();
        let queued = Arc::new(Queued {
            bytes: AtomicUsize::new(0),
            overrun: AtomicBool::new(false),
            writer_ended: Mutex::new(Some(writer_ended)),
            patience: farewell.as_ref().map(|farewell| farewell.patience),
        });
        let mut writing = stream.try_clone()?;
        let written = Arc::clone(&queued);
        std::thread::Builder::new().name(format!("write {id}")).spawn(move || {
            let _ending = ending;
            let mut reached = Position::default();
            for outbound in receiver {
                if written.overrun.load(Ordering::Acquire) {
                    break;
                }
                match outbound {
                    Outbound::Frame(frame, position) => {
                        if muster_frame::write_frame(&mut writing, &frame).is_err() {
                            let _ = writing.shutdown(Shutdown::Both);
                            return;
                        }
                        written.bytes.fetch_sub(frame.len(), Ordering::AcqRel);
                        if let Some(position) = position {
                            reached = position;
                        }
                    }
                    Outbound::Answer(frame, answered) => {
                        if muster_frame::write_frame(&mut writing, &frame).is_err() {
                            let _ = writing.shutdown(Shutdown::Both);
                            return;
                        }
                        written.bytes.fetch_sub(frame.len(), Ordering::AcqRel);
                        answered.log(id);
                    }
                    Outbound::Flushed(done) => {
                        let _ = done.send(());
                    }
                    Outbound::Overrun => break,
                }
            }
            if written.overrun.load(Ordering::Acquire) {
                if let Some(farewell) = farewell {
                    let _ = muster_frame::write_frame(&mut writing, &(farewell.words)(reached));
                }
                let _ = writing.shutdown(Shutdown::Both);
            }
        })?;
        Ok(Outbox { id, sender, stream: Arc::new(stream.try_clone()?), budget, queued })
    }

    /// Queues a frame. False when the connection is gone or has fallen too far behind, in which
    /// case it is hung up.
    pub(crate) fn push(&self, frame: Arc<[u8]>) -> bool {
        self.queue(Outbound::Frame(frame, None), false)
    }

    /// Queues a frame that stands at `position` in the connection's numbered stream, which is
    /// what its farewell says was reached once the frame is written.
    pub(crate) fn push_at(&self, frame: Arc<[u8]>, position: Position) -> bool {
        self.queue(Outbound::Frame(frame, Some(position)), false)
    }

    fn queue(&self, outbound: Outbound, quiet: bool) -> bool {
        if self.queued.overrun.load(Ordering::Acquire) {
            return false;
        }
        let bytes = match &outbound {
            Outbound::Frame(frame, _) | Outbound::Answer(frame, _) => frame.len(),
            Outbound::Flushed(_) | Outbound::Overrun => 0,
        };
        let queued = self.queued.bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        if queued <= self.budget.bytes {
            match self.sender.try_send(outbound) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(_)) => {}
            }
        }
        self.fell_behind(queued, quiet);
        false
    }

    /// Gives up on a connection that has more waiting than its budget: hung up on now, or once
    /// its farewell is written.
    fn fell_behind(&self, queued: usize, quiet: bool) {
        if self.queued.overrun.swap(true, Ordering::AcqRel) {
            return;
        }
        if !quiet {
            log::warn(
                "daemon.connection.overrun",
                fields! {
                    "connection" => self.id,
                    "budget" => self.budget,
                    "bytes" => queued,
                    "impact" => "the connection is dropped; its client misses nothing it \
                                 cannot recover by subscribing again",
                    "check" => "whether the client stopped reading its socket",
                },
            );
        }
        let Some(patience) = self.queued.patience else {
            let _ = self.stream.shutdown(Shutdown::Both);
            return;
        };
        let _ = self.sender.try_send(Outbound::Overrun);
        let ended =
            poison::lock(&self.queued.writer_ended, "daemon.connection.writer_ended").take();
        let stream = Arc::clone(&self.stream);
        // A writer blocked on a client that never reads again is unblocked only by this.
        let _ = std::thread::Builder::new().name(format!("hang up {}", self.id)).spawn(move || {
            if let Some(ended) = ended
                && ended.recv_timeout(patience) == Err(mpsc::RecvTimeoutError::Timeout)
            {
                let _ = stream.shutdown(Shutdown::Both);
            }
        });
    }

    /// Queues a frame as [`Outbox::push`] does, and says nothing when it cannot: the daemon's
    /// log uses this to hand on its own records, and a warning from here would come straight
    /// back into the log. A connection that far behind is still hung up, and its closing is
    /// logged by the thread serving it.
    pub(crate) fn offer(&self, frame: Arc<[u8]>) -> bool {
        self.queue(Outbound::Frame(frame, None), true)
    }

    /// Waits until everything queued so far has been written, or `within` has passed.
    pub(crate) fn flush(&self, within: Duration) {
        let (done, flushed) = mpsc::channel();
        if self.sender.try_send(Outbound::Flushed(done)).is_ok() {
            let _ = flushed.recv_timeout(within);
        }
    }
}

/// Where one request's time went, from reading it to writing its answer.
///
/// Logged by the writer, once the answer is on the socket, because that is the moment its client
/// can have it: an answer queued behind a slow client's backlog has not been given yet.
#[derive(Debug, Clone, Copy)]
struct Answered {
    id: u64,
    request: &'static str,
    received: Instant,
    /// Waiting for the session lock, which every request takes and anything holding it delays.
    lock: Duration,
    queued: Instant,
}

impl Answered {
    fn after(id: u64, request: &'static str, received: Instant, lock: Duration) -> Answered {
        Answered { id, request, received, lock, queued: received }
    }

    fn log(&self, connection: u64) {
        let ms = |elapsed: Duration| format!("{:.1}", elapsed.as_secs_f64() * 1000.0);
        log::debug(
            "daemon.request.answered",
            fields! {
                "connection" => connection,
                "id" => self.id,
                "request" => self.request,
                "ms" => ms(self.received.elapsed()),
                "lock_ms" => ms(self.lock),
                "queued_ms" => ms(self.queued.elapsed()),
            },
        );
    }
}

/// Serves requests on a connection that has already been welcomed, until it hangs up.
#[expect(clippy::too_many_lines, reason = "one arm per way a request is answered")]
pub(crate) fn serve(mut stream: UnixStream, shared: &Arc<Shared>, client: &str) {
    let outbox = match Outbox::open(&stream) {
        Ok(outbox) => outbox,
        Err(error) => {
            log::error(
                "daemon.connection.not_served",
                fields! {
                    "client" => client,
                    "error" => error,
                    "impact" => "this client's connection is closed unanswered",
                    "check" => "whether the daemon is out of threads or descriptors",
                },
            );
            return;
        }
    };
    log::info(
        "daemon.connection.opened",
        fields! { "connection" => outbox.id, "client" => client },
    );
    let peer = attribution::peer_pid(&stream);
    let reader_ended = Arc::new(AtomicBool::new(false));

    loop {
        let request = match connection::receive::<proto::Request>(&mut stream) {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(error) => {
                log::warn(
                    "daemon.connection.unreadable",
                    fields! {
                        "connection" => outbox.id,
                        "error" => error,
                        "impact" => "the connection is closed; its client has to reconnect",
                        "check" => "whether the client speaks this daemon's protocol version",
                    },
                );
                break;
            }
        };
        let received = Instant::now();
        let name = request.service.as_ref().map_or("none", muster_daemon_proto::service_name);
        let mut lock = Duration::ZERO;
        let mut locked = || {
            let asked = Instant::now();
            let session = shared.lock();
            lock += asked.elapsed();
            session
        };
        let mut stop = matches!(
            &request.service,
            Some(Service::Session(proto::SessionRequest {
                request: Some(session_request::Request::Stop(_))
            }))
        )
        .then_some(Stop::Asked);
        // The events a request produces are queued under this lock. Its answer is queued under
        // the next, once letting go of this one has done the work it left on panes' terminals,
        // so an answer still means the request has taken effect. A snapshot leaves no such work,
        // and its answer goes under this lock, beside the state it describes.
        let sender = sender_of(peer, request.service.as_ref(), shared);
        let handled = match request.service {
            // Outside the session's lock: messages have their own (crate::messages).
            Some(Service::Msg(message)) => {
                let handled = match &sender {
                    Some(attribution::Sender::Server(server)) => {
                        messages::Handled::Reply(messages::through_server(&message, server))
                    }
                    Some(attribution::Sender::Agent(nearest)) => {
                        match from_agent(shared, message, nearest) {
                            Ok(message) => messages::handle(shared, message, &|| hung_up(&stream)),
                            Err(refused) => messages::Handled::Reply(refused),
                        }
                    }
                    None => messages::handle(shared, message, &|| hung_up(&stream)),
                };
                match handled {
                    messages::Handled::Reply(reply) => Handled::Reply(reply),
                    messages::Handled::Carrying(carrying) => {
                        let answered = Answered::after(request.id, name, received, lock);
                        carry_later(shared, &outbox, *carrying, &reader_ended, answered);
                        continue;
                    }
                }
            }
            Some(service) => {
                let mut session = locked();
                match handle(&mut session, service, sender.as_ref(), &outbox, shared) {
                    Handled::Snapshot(reply) => {
                        let timing = Answered::after(request.id, name, received, lock);
                        answer(&session, &outbox, reply, timing);
                        continue;
                    }
                    handled => handled,
                }
            }
            None => Handled::Reply(Reply::unsupported()),
        };
        let reply = match handled {
            Handled::Reply(reply) | Handled::Snapshot(reply) => reply,
            // Outside the lock: starting a process waits for it to change directory and exec,
            // and a directory on a hung mount would otherwise stall every connection with it.
            Handled::Start(mut starting) => {
                let started = starting.start();
                locked().started(*starting, started)
            }
            Handled::Read(reading) => reading.read(),
            // Outside the lock too: compiling reads the override directory, which can hang.
            Handled::Manifests(loading) => {
                let loaded = loading.load();
                locked().manifests_loaded(*loading, loaded)
            }
            // Off this connection's thread as well as outside the lock: it waits seconds on a
            // program to end, and the connection's next request should not wait with it. An
            // answer carries its request's id, so this one may follow later requests' answers.
            Handled::Restart(restarting) => {
                let answered = Answered::after(request.id, name, received, lock);
                answer_later(shared, &outbox, *restarting, answered);
                continue;
            }
            // Outside the lock: it waits on another daemon, step by step.
            Handled::Replace(replacement) => {
                let (reply, handed_off) = hand_over(shared, &replacement);
                stop = handed_off.or(stop);
                reply
            }
        };
        let session = locked();
        answer(&session, &outbox, reply, Answered::after(request.id, name, received, lock));
        drop(session);
        if let Some(stop) = stop {
            reader_ended.store(true, Ordering::Release);
            outbox.flush(STOP_FLUSH);
            let _ = shared.stopping.send(stop);
            return;
        }
    }

    reader_ended.store(true, Ordering::Release);
    shared.lock().unsubscribe(outbox.id);
    let _ = stream.shutdown(Shutdown::Both);
    log::info("daemon.connection.closed", fields! { "connection" => outbox.id });
}

/// Handles a request under the session's lock. One that renames a pane, reports a session's
/// name or its context, asks for a compaction, or turns naming sessions or compacting on may
/// leave a line to type into an agent's prompt, which the doorbell's thread does - but only if
/// it changed something: a statusline repeats its report every few seconds. A report whose
/// `sender` is another agent than the pane's own is refused before any of it is taken.
fn handle(
    session: &mut Session,
    service: Service,
    sender: Option<&attribution::Sender>,
    outbox: &Outbox,
    shared: &Shared,
) -> Handled {
    if let Some(refused) = sender.and_then(|sender| session.foreign_report(&service, sender)) {
        return Handled::Reply(refused);
    }
    let names = matches!(
        &service,
        Service::Pane(proto::PaneRequest {
            request: Some(
                pane_request::Request::Rename(_)
                    | pane_request::Request::Report(_)
                    | pane_request::Request::Compact(_)
            )
        }) | Service::Session(proto::SessionRequest {
            request: Some(
                session_request::Request::SetNameSessions(_)
                    | session_request::Request::SetCompactAt(_)
            )
        })
    );
    let handled = session.handle(service, outbox);
    if names && matches!(&handled, Handled::Reply(reply) if reply.outcome == proto::Outcome::Done) {
        shared.doorbell.nudge();
    }
    handled
}

/// The nearest agent or server above the sender of a report, or of a msg request that names a
/// pane or an inbox; read before the session's lock is taken, since the walk reads the process
/// table a process at a time. Waits briefly for a starting daemon's manifests, since a hook can
/// report the moment a restarted daemon listens.
fn sender_of(
    peer: Option<u32>,
    service: Option<&Service>,
    shared: &Shared,
) -> Option<attribution::Sender> {
    let asked = attributed(service)?;
    let peer = peer?;
    let Some(manifests) = shared.detecting.manifests_within(MANIFESTS_PATIENCE) else {
        attribution::unattributed(peer, MANIFESTS_PATIENCE, asked);
        return None;
    };
    attribution::sender(peer, &manifests)
}

/// Whether `service` names who it is from in a way a walk can contradict. A msg request naming
/// neither a pane nor an inbox is the person's, which nothing here ties to a process, and a peer
/// daemon's link names neither.
fn attributed(service: Option<&Service>) -> Option<attribution::Asked> {
    match service? {
        Service::Pane(proto::PaneRequest { request: Some(pane_request::Request::Report(_)) }) => {
            Some(attribution::Asked::Report)
        }
        Service::Msg(request) => request
            .caller
            .as_ref()
            .filter(|caller| {
                caller.pane.as_deref().is_some_and(|pane| !pane.is_empty())
                    || caller.inbox.is_some()
            })
            .map(|_| attribution::Asked::Request),
        _ => None,
    }
}

/// A msg request from `nearest`, as the daemon takes it. Sent by an agent nested in the pane it
/// names, such as a `claude -p` the pane's agent started, a read or a wait as the pane is refused,
/// since the nested agent's messaging hooks would take the pane's agent's messages, and anything
/// else is narrowed to the addresses that are its to give ([`messages::as_nested`]). A nested
/// session whose own inbox is a participant's, having joined under a name of its own, reads and
/// posts as that participant. Any other request is taken as it came.
fn from_agent(
    shared: &Shared,
    request: proto::MsgRequest,
    nearest: &attribution::Nearest,
) -> Result<proto::MsgRequest, Reply> {
    use proto::msg_request::Request;
    let Some(caller) = request.caller.as_ref() else { return Ok(request) };
    let Some(pane) = caller.pane.clone().filter(|pane| !pane.is_empty()) else {
        return Ok(request);
    };
    let Some(own) = shared.lock().nested_in(&pane, nearest).map(str::to_string) else {
        return Ok(request);
    };
    let its_own =
        caller.inbox.as_ref().is_some_and(|inbox| messages::inbox_of_its_own(shared, inbox, &pane));
    let receives = matches!(request.request, Some(Request::Read(_) | Request::Wait(_)));
    if receives && caller.as_name.is_none() && !its_own {
        return Err(messages::from_nested(&pane, &own, nearest));
    }
    Ok(messages::as_nested(request, its_own))
}

/// Hands this daemon's panes to a replacement, with messages refusing changes until it is
/// decided, and says whether this daemon stops because of it.
fn hand_over(shared: &Arc<Shared>, replacement: &Replacement) -> (Reply, Option<Stop>) {
    shared.messages().handing_over(true);
    shared.peers.close_all();
    let reply = handoff::hand_over(shared, replacement);
    if reply.outcome == proto::Outcome::Done {
        return (reply, Some(Stop::HandedOff));
    }
    shared.messages().handing_over(false);
    shared.doorbell.nudge();
    (reply, None)
}

/// Whether the client has closed its end: a read that would not wait finds the end of the
/// stream. Peeked, so a request it sent meanwhile stays to be read, and without changing the
/// socket's blocking mode, which its writer thread shares.
fn hung_up(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut byte = 0u8;
    // SAFETY: `byte` is a valid, writable one-byte buffer for the length given, and the
    // descriptor is the stream's, open for as long as `stream` is borrowed.
    let read = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&raw mut byte).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    // A reset connection reads as an error rather than as the end; only "nothing yet" is not
    // a caller gone.
    read == 0
        || (read < 0
            && !matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EAGAIN | libc::EINTR)
            ))
}

/// Runs a restart on a thread of its own, and answers it once it is done.
fn answer_later(shared: &Arc<Shared>, outbox: &Outbox, restarting: Restarting, answered: Answered) {
    let (shared, outbox) = (Arc::clone(shared), outbox.clone());
    let name = format!("restart {}", restarting.pane);
    let started = std::thread::Builder::new().name(name).spawn({
        let outbox = outbox.clone();
        let shared = Arc::clone(&shared);
        move || {
            let reply = restarting.run();
            answer(&shared.lock(), &outbox, reply, answered);
        }
    });
    if let Err(error) = started {
        let reply = Reply::refused(format!(
            "the daemon could not start a thread to restart the agent ({error}), so nothing was \
             done"
        ));
        answer(&shared.lock(), &outbox, reply, answered);
    }
}

fn carry_later(
    shared: &Arc<Shared>,
    outbox: &Outbox,
    carrying: messages::Carrying,
    reader_ended: &Arc<AtomicBool>,
    answered: Answered,
) {
    let (shared, outbox) = (Arc::clone(shared), outbox.clone());
    let reader_ended = Arc::clone(reader_ended);
    let started = std::thread::Builder::new().name(format!("carry {}", answered.id)).spawn({
        let shared = Arc::clone(&shared);
        let outbox = outbox.clone();
        move || {
            let gone = || reader_ended.load(Ordering::Acquire) || hung_up(&outbox.stream);
            let reply = carrying.run(&shared, &gone);
            let waiting = Instant::now();
            let session = shared.lock();
            let answered = Answered { lock: answered.lock + waiting.elapsed(), ..answered };
            answer(&session, &outbox, reply, answered);
        }
    });
    if let Err(error) = started {
        let reply = Reply::refused(format!(
            "the daemon could not start a thread to carry this request ({error}); it was not \
             sent to the other machine"
        ));
        let waiting = Instant::now();
        let session = shared.lock();
        let answered = Answered { lock: answered.lock + waiting.elapsed(), ..answered };
        answer(&session, &outbox, reply, answered);
    }
}

/// Queues the answer to a request. Called with the session locked, after the request's events
/// were queued, so they reach a subscriber first.
fn answer(session: &Session, outbox: &Outbox, reply: Reply, answered: Answered) {
    let id = answered.id;
    let answer = proto::Answer {
        id,
        outcome: reply.outcome.into(),
        reason: reply.reason,
        seq: session.seq(),
        detail: reply.detail.map(|detail| *detail),
    };
    let wrap = |answer| proto::ControlMessage {
        message: Some(proto::control_message::Message::Answer(answer)),
    };
    let mut frame = wrap(answer.clone()).encode_to_vec();
    // The client would refuse the frame and drop the connection, and its subscription with it.
    if frame.len() > connection::LARGEST_MESSAGE as usize {
        let reason = format!(
            "the answer would be {} bytes, more than the {} a message may be",
            frame.len(),
            connection::LARGEST_MESSAGE
        );
        log::error(
            "daemon.connection.answer_too_large",
            fields! {
                "connection" => outbox.id,
                "request" => id,
                "bytes" => frame.len(),
                "impact" => "the request is answered refused instead",
                "check" => "this is a bug: whatever built the answer should have bounded it",
            },
        );
        frame = wrap(proto::Answer {
            outcome: proto::Outcome::Refused.into(),
            reason,
            detail: None,
            ..answer
        })
        .encode_to_vec();
    }
    outbox.queue(
        Outbound::Answer(frame.into(), Answered { queued: Instant::now(), ..answered }),
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = 1024;

    /// A frame of `FRAME` bytes saying which it was.
    fn numbered(number: u64) -> Arc<[u8]> {
        let mut frame = vec![0u8; FRAME];
        frame[..8].copy_from_slice(&number.to_le_bytes());
        frame.into()
    }

    fn number_of(frame: &[u8]) -> u64 {
        u64::from_le_bytes(frame[..8].try_into().unwrap())
    }

    fn farewell(patience: Duration) -> Farewell {
        Farewell {
            words: Box::new(|at: Position| format!("bye {} {}", at.epoch, at.number).into_bytes()),
            patience,
        }
    }

    /// Pushes numbered frames at epoch 7 until the outbox refuses one, which it has to within
    /// far more than any kernel buffers, and returns how many it took.
    fn push_until_refused(outbox: &Outbox) -> u64 {
        for number in 1..=100_000 {
            if !outbox.push_at(numbered(number), Position { epoch: 7, number }) {
                return number - 1;
            }
        }
        panic!("the outbox never refused a frame from a client that reads nothing");
    }

    /// Everything the client end reads until the daemon's end hangs up.
    fn read_to_end(client: &mut UnixStream) -> Vec<Vec<u8>> {
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut frames = Vec::new();
        while let Ok(Some(frame)) = muster_frame::read_frame_or_end(client, 1 << 24) {
            frames.push(frame);
        }
        frames
    }

    /// The frames read before the farewell say where the client got to, and the farewell says
    /// the same: nothing, when the budget ran out before the writer wrote a frame.
    fn check_farewell(frames: &[Vec<u8>]) {
        let (last, numbered) = frames.split_last().expect("something was read");
        let numbers: Vec<u64> = numbered.iter().map(|frame| number_of(frame)).collect();
        let reached = numbers.last().copied().unwrap_or(0);
        assert_eq!(numbers, (1..=reached).collect::<Vec<_>>(), "the frames written, in order");
        let epoch = if reached == 0 { 0 } else { 7 };
        assert_eq!(String::from_utf8_lossy(last), format!("bye {epoch} {reached}"));
    }

    #[test]
    fn a_connection_past_its_byte_budget_is_told_where_it_got_to_and_hung_up_on() {
        let (daemon_end, mut client) = UnixStream::pair().unwrap();
        let budget = Budget { messages: 100_000, bytes: 8 * FRAME };
        let outbox =
            Outbox::open_with(&daemon_end, budget, Some(farewell(Duration::from_secs(10))))
                .unwrap();
        push_until_refused(&outbox);
        assert!(!outbox.push(numbered(0)), "nothing more is queued once it fell behind");
        check_farewell(&read_to_end(&mut client));
    }

    #[test]
    fn a_connection_past_its_message_budget_is_told_where_it_got_to_and_hung_up_on() {
        let (daemon_end, mut client) = UnixStream::pair().unwrap();
        let budget = Budget { messages: 4, bytes: usize::MAX };
        let outbox =
            Outbox::open_with(&daemon_end, budget, Some(farewell(Duration::from_secs(10))))
                .unwrap();
        push_until_refused(&outbox);
        check_farewell(&read_to_end(&mut client));
    }

    /// A client that never reads again cannot hold the writer forever: it is hung up on without
    /// its farewell once the patience has passed. A frame larger than any socket buffer is what
    /// holds the writer.
    #[test]
    fn a_connection_that_never_reads_again_is_hung_up_on_without_its_farewell() {
        let (daemon_end, mut client) = UnixStream::pair().unwrap();
        let budget = Budget { messages: 4, bytes: usize::MAX };
        let outbox =
            Outbox::open_with(&daemon_end, budget, Some(farewell(Duration::from_millis(100))))
                .unwrap();
        let mut number = 0;
        while outbox.push(vec![0u8; 4 << 20].into()) {
            number += 1;
            assert!(number < 100, "the outbox never refused a frame");
        }
        muster_harness::until(
            "the writer to be hung up on",
            || Arc::strong_count(&outbox.queued) == 1,
            (),
        );
        let frames = read_to_end(&mut client);
        assert!(
            frames.iter().all(|frame| !frame.starts_with(b"bye")),
            "the farewell was written to a client that was not reading"
        );
    }

    #[test]
    fn a_control_connection_is_hung_up_on_at_once_and_told_nothing() {
        let (daemon_end, mut client) = UnixStream::pair().unwrap();
        let outbox = Outbox::open(&daemon_end).unwrap();
        push_until_refused(&outbox);
        let frames = read_to_end(&mut client);
        assert!(frames.iter().all(|frame| frame.len() == FRAME), "only the frames queued");
    }
}
