//! One pane's bytes, from the daemon to a surface.
//!
//! A bridge attaches to a pane, writes what the daemon sends to its surface, and acknowledges
//! the output it wrote, which is what keeps the daemon sending: the daemon holds at most a window
//! of unacknowledged output per pane, and a bridge that stops acknowledging falls behind and is
//! caught up with the screen once it credits again (MIP-3, section 4).

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use muster_daemon_proto::connection::{self, HandshakeError};
use muster_daemon_proto::{
    self as proto, ConnectionKind, DetachReason, Grid, stream_message, stream_request,
};

/// A stream attached to one pane, before its first byte is written.
#[derive(Debug)]
pub struct Attachment {
    reading: UnixStream,
    writing: Arc<Mutex<UnixStream>>,
    pane: String,
}

/// Why an attach did not happen.
#[derive(Debug)]
pub enum AttachError {
    /// No daemon answered on the socket, or it would not talk to this client.
    Handshake(HandshakeError),
    /// The daemon answered and would not attach, for the reason it gave.
    Refused(String),
    /// The daemon went away or answered with something other than an attach's answer.
    Broken(String),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::Handshake(error) => write!(formatter, "{error}"),
            AttachError::Refused(reason) => write!(formatter, "the daemon refused: {reason}"),
            AttachError::Broken(why) => write!(formatter, "the attach did not complete: {why}"),
        }
    }
}

impl std::error::Error for AttachError {}

/// How a pump ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The daemon said the pane is no longer this stream's.
    Detached(DetachReason),
    /// The daemon hung up without saying why.
    HungUp,
    /// Writing to the surface or reading from the daemon failed.
    Failed(String),
}

/// What happened while pumping that a bridge may want to say.
#[derive(Debug, PartialEq, Eq)]
pub enum Happened {
    /// The daemon stopped sending output because the surface had not acknowledged a window of
    /// it. Output resumes after a catch-up.
    Behind,
    /// A catch-up of this many bytes, all of them written, after falling behind.
    CaughtUp(usize),
}

impl Attachment {
    /// Dials `socket` and attaches to `pane` at `grid`, displacing another bridge if `takeover`.
    ///
    /// Returns the pane's output offset the replay that follows is current to. `client` says
    /// who is asking, for the daemon's log.
    pub fn open(
        socket: &Path,
        pane: &str,
        grid: Grid,
        takeover: bool,
        client: &str,
    ) -> Result<(Attachment, u64), AttachError> {
        Attachment::open_with_window(socket, pane, grid, takeover, None, client)
    }

    /// As [`Attachment::open`], asking for a window of `window` bytes of unacknowledged output
    /// rather than the daemon's default: more for a bridge across a slow link, whose pane's
    /// program is held to about one window per round trip.
    pub fn open_with_window(
        socket: &Path,
        pane: &str,
        grid: Grid,
        takeover: bool,
        window: Option<u64>,
        client: &str,
    ) -> Result<(Attachment, u64), AttachError> {
        let (mut stream, _) = connection::connect(socket, ConnectionKind::Stream, client)
            .map_err(AttachError::Handshake)?;
        let attach =
            stream_request::Attach { pane: pane.to_string(), grid: Some(grid), takeover, window };
        send(&mut stream, stream_request::Request::Attach(attach))
            .map_err(|error| AttachError::Broken(error.to_string()))?;
        let offset = match connection::receive::<proto::StreamMessage>(&mut stream) {
            Ok(Some(proto::StreamMessage {
                message: Some(stream_message::Message::Attached(attached)),
            })) => attached.offset,
            Ok(Some(proto::StreamMessage {
                message: Some(stream_message::Message::Refused(refused)),
            })) => return Err(AttachError::Refused(refused.reason)),
            Ok(Some(other)) => {
                return Err(AttachError::Broken(format!("the daemon answered {other:?}")));
            }
            Ok(None) => {
                return Err(AttachError::Broken("the daemon hung up before answering".into()));
            }
            Err(error) => return Err(AttachError::Broken(error)),
        };
        let writing = stream.try_clone().map_err(|error| AttachError::Broken(error.to_string()))?;
        let attachment = Attachment {
            reading: stream,
            writing: Arc::new(Mutex::new(writing)),
            pane: pane.into(),
        };
        Ok((attachment, offset))
    }

    /// A handle that tells the daemon the surface's grid changed, from any thread.
    pub fn resizer(&self) -> Resizer {
        Resizer { writing: Arc::clone(&self.writing) }
    }

    /// Writes everything the daemon sends to `surface` until the stream ends, acknowledging
    /// output as it is written. `happened` hears about falling behind and catching up.
    ///
    /// A catch-up is reported once all of it has been written. It can arrive in several replay
    /// pieces and nothing marks its last, so it is over when the daemon sends anything else, or
    /// hangs up.
    pub fn pump(mut self, surface: &mut impl Write, mut happened: impl FnMut(Happened)) -> Ended {
        let mut behind = false;
        // Bytes of a catch-up written so far, while one is arriving.
        let mut catching_up: Option<usize> = None;
        loop {
            let message = match connection::receive::<proto::StreamMessage>(&mut self.reading) {
                Ok(Some(message)) => message.message,
                Ok(None) => {
                    if let Some(total) = catching_up {
                        happened(Happened::CaughtUp(total));
                    }
                    return Ended::HungUp;
                }
                Err(error) => {
                    return Ended::Failed(format!("reading {}'s stream: {error}", self.pane));
                }
            };
            if !matches!(message, Some(stream_message::Message::Replay(_)))
                && let Some(total) = catching_up.take()
            {
                happened(Happened::CaughtUp(total));
            }
            let (bytes, credit) = match message {
                Some(stream_message::Message::Replay(bytes)) => (bytes, false),
                Some(stream_message::Message::Output(bytes)) => (bytes, true),
                Some(stream_message::Message::Behind(_)) => {
                    behind = true;
                    happened(Happened::Behind);
                    continue;
                }
                Some(stream_message::Message::Detached(detached)) => {
                    return Ended::Detached(detached.reason());
                }
                // Neither comes after an attach; skipped rather than ending a pane's stream over
                // a message that says nothing about its bytes.
                Some(
                    stream_message::Message::Attached(_) | stream_message::Message::Refused(_),
                )
                | None => continue,
            };
            if let Err(error) = surface.write_all(&bytes).and_then(|()| surface.flush()) {
                return Ended::Failed(format!(
                    "writing {}'s bytes to the surface: {error}",
                    self.pane
                ));
            }
            if credit {
                let credit = stream_request::Credit { bytes: bytes.len() as u64 };
                let mut writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
                // A daemon that stopped reading has hung up, and the next read says so.
                let _ = send(&mut writing, stream_request::Request::Credit(credit));
            } else if std::mem::take(&mut behind) || catching_up.is_some() {
                *catching_up.get_or_insert(0) += bytes.len();
            }
        }
    }
}

/// Tells the daemon the surface's grid changed. See [`Attachment::resizer`].
#[derive(Debug, Clone)]
pub struct Resizer {
    writing: Arc<Mutex<UnixStream>>,
}

impl Resizer {
    /// Fails only when the daemon has hung up, which the pump reports on its own.
    pub fn resize(&self, grid: Grid) -> std::io::Result<()> {
        let mut writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        send(
            &mut writing,
            stream_request::Request::Resize(stream_request::Resize { grid: Some(grid) }),
        )
    }
}

fn send(stream: &mut UnixStream, request: stream_request::Request) -> std::io::Result<()> {
    connection::send(stream, &proto::StreamRequest { request: Some(request) })
}
