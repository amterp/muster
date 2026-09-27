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
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, request::Service, session_request};
use prost::Message;

use crate::session::{Handled, Reply, Session, Shared};

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
    /// Answered once everything queued before it has been written.
    Flushed(mpsc::Sender<()>),
}

impl Outbox {
    fn open(stream: &UnixStream) -> std::io::Result<Outbox> {
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
        match self.sender.try_send(Outbound::Frame(frame)) {
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

    /// Waits until everything queued so far has been written, or `within` has passed.
    fn flush(&self, within: Duration) {
        let (done, flushed) = mpsc::channel();
        if self.sender.try_send(Outbound::Flushed(done)).is_ok() {
            let _ = flushed.recv_timeout(within);
        }
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
        let stopping = matches!(
            &request.service,
            Some(Service::Session(proto::SessionRequest {
                request: Some(session_request::Request::Stop(_))
            }))
        );
        // The events a request produces are queued under this lock. Its answer is queued under
        // the next, once letting go of this one has done the work it left on panes' terminals,
        // so an answer still means the request has taken effect.
        let handled = match request.service {
            Some(service) => shared.lock().handle(service, &outbox),
            None => Handled::Reply(Reply::unsupported()),
        };
        let reply = match handled {
            Handled::Reply(reply) => reply,
            // Outside the lock: starting a process waits for it to change directory and exec,
            // and a directory on a hung mount would otherwise stall every connection with it.
            Handled::Start(starting) => {
                let started = starting.start();
                shared.lock().started(*starting, started)
            }
            Handled::Read(reading) => reading.read(),
            // Outside the lock too: compiling reads the override directory, which can hang.
            Handled::Manifests(loading) => {
                let loaded = loading.load();
                shared.lock().manifests_loaded(*loading, loaded)
            }
        };
        answer(&shared.lock(), &outbox, request.id, reply);
        if stopping {
            outbox.flush(STOP_FLUSH);
            let _ = shared.stopping.send(());
            return;
        }
    }

    shared.lock().unsubscribe(outbox.id);
    let _ = stream.shutdown(Shutdown::Both);
    log::info("daemon.connection.closed", fields! { "connection" => outbox.id });
}

/// Queues the answer to request `id`. Called with the session locked, after the request's
/// events were queued, so they reach a subscriber first.
fn answer(session: &Session, outbox: &Outbox, id: u64, reply: Reply) {
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
    outbox.push(frame.into());
}
