//! A control connection: requests in, answers and events out.
//!
//! Everything a connection sends goes through its [`Outbox`], a bounded queue drained by a
//! writer thread of its own. Queuing never blocks, so the session lock is never held waiting on
//! a slow client's socket; a client too slow to keep a queue this deep is dropped instead, and
//! subscribes again when it reconnects.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, atomic::AtomicU64, atomic::Ordering};
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, request::Service, session_request};
use prost::Message;

use crate::handoff;
use crate::session::{Handled, Reply, Session, Shared, Stop};

/// How many messages a connection may have waiting before the daemon gives up on it.
///
/// A client reading at all keeps this near empty: events are a few hundred bytes and arrive
/// at the rate panes change. Filling it means the client has stopped reading, and holding more
/// for it would only grow the daemon.
const QUEUE_DEPTH: usize = 4096;

/// How long a `stop` waits for its answer to be written before the daemon exits anyway.
const STOP_FLUSH: Duration = Duration::from_secs(2);

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// What a connection has to send, in order.
#[derive(Debug, Clone)]
pub(crate) struct Outbox {
    pub(crate) id: u64,
    sender: SyncSender<Outbound>,
    stream: Arc<UnixStream>,
}

#[derive(Debug)]
enum Outbound {
    Frame(Arc<[u8]>),
    /// A request's answer, and how it came about, logged once it is written.
    Answer(Arc<[u8]>, Answered),
    /// Answered once everything queued before it has been written.
    Flushed(mpsc::Sender<()>),
}

impl Outbox {
    pub(crate) fn open(stream: &UnixStream) -> std::io::Result<Outbox> {
        let id = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
        let mut writing = stream.try_clone()?;
        std::thread::Builder::new().name(format!("write {id}")).spawn(move || {
            for outbound in receiver {
                match outbound {
                    Outbound::Frame(frame) => {
                        if muster_frame::write_frame(&mut writing, &frame).is_err() {
                            let _ = writing.shutdown(Shutdown::Both);
                            return;
                        }
                    }
                    Outbound::Answer(frame, answered) => {
                        if muster_frame::write_frame(&mut writing, &frame).is_err() {
                            let _ = writing.shutdown(Shutdown::Both);
                            return;
                        }
                        answered.log(id);
                    }
                    Outbound::Flushed(done) => {
                        let _ = done.send(());
                    }
                }
            }
        })?;
        Ok(Outbox { id, sender, stream: Arc::new(stream.try_clone()?) })
    }

    /// Queues a frame. False when the connection is gone or has fallen too far behind, in which
    /// case it is hung up.
    pub(crate) fn push(&self, frame: Arc<[u8]>) -> bool {
        self.queue(Outbound::Frame(frame))
    }

    fn queue(&self, outbound: Outbound) -> bool {
        match self.sender.try_send(outbound) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                log::warn(
                    "daemon.connection.overrun",
                    fields! {
                        "connection" => self.id,
                        "queued" => QUEUE_DEPTH,
                        "impact" => "the connection is dropped; its client misses nothing it \
                                     cannot recover by subscribing again",
                        "check" => "whether the client stopped reading its socket",
                    },
                );
                let _ = self.stream.shutdown(Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Queues a frame as [`Outbox::push`] does, and says nothing when it cannot: the daemon's
    /// log uses this to hand on its own records, and a warning from here would come straight
    /// back into the log. A connection that far behind is still hung up, and its closing is
    /// logged by the thread serving it.
    pub(crate) fn offer(&self, frame: Arc<[u8]>) -> bool {
        match self.sender.try_send(Outbound::Frame(frame)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                let _ = self.stream.shutdown(Shutdown::Both);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
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
        let handled = match request.service {
            Some(service) => {
                let mut session = locked();
                match session.handle(service, &outbox) {
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
            Handled::Start(starting) => {
                let started = starting.start();
                locked().started(*starting, started)
            }
            Handled::Read(reading) => reading.read(),
            // Outside the lock too: compiling reads the override directory, which can hang.
            Handled::Manifests(loading) => {
                let loaded = loading.load();
                locked().manifests_loaded(*loading, loaded)
            }
            // Outside the lock: it waits on another daemon, step by step.
            Handled::Replace(replacement) => {
                let reply = handoff::hand_over(shared, &replacement);
                if reply.outcome == proto::Outcome::Done {
                    stop = Some(Stop::HandedOff);
                }
                reply
            }
        };
        let session = locked();
        answer(&session, &outbox, reply, Answered::after(request.id, name, received, lock));
        drop(session);
        if let Some(stop) = stop {
            outbox.flush(STOP_FLUSH);
            let _ = shared.stopping.send(stop);
            return;
        }
    }

    shared.lock().unsubscribe(outbox.id);
    let _ = stream.shutdown(Shutdown::Both);
    log::info("daemon.connection.closed", fields! { "connection" => outbox.id });
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
    outbox.queue(Outbound::Answer(frame.into(), Answered { queued: Instant::now(), ..answered }));
}
