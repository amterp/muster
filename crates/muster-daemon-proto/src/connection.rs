//! Carrying the protocol's messages, and opening a connection the way the daemon expects.
//!
//! Every connection starts with a handshake (the schema's header): the client sends a `Hello`
//! and reads a `HelloAnswer` before anything else. This is the client's half, for the app, the
//! bridge and the test harness alike, so there is one of it.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use prost::Message;

use crate::version::PROTOCOL;
use crate::{ConnectionKind, Hello, HelloRefused, Welcome, hello_answer};

/// The most a message either way may be.
///
/// Larger than the command socket's megabyte because this protocol carries a page of a pane's
/// text and a set of detection manifests, both of which a person can make large. Still a ceiling:
/// a peer announcing more than this is not a Muster peer, and is refused before anything is
/// allocated for it.
pub const LARGEST_MESSAGE: u32 = 16 << 20;

/// Writes one message as a frame.
pub fn send(stream: &mut impl Write, message: &impl Message) -> std::io::Result<()> {
    muster_frame::write_frame(stream, &message.encode_to_vec())
}

/// Reads one message, or `None` when the other end hung up between messages.
pub fn receive<M: Message + Default>(stream: &mut impl Read) -> Result<Option<M>, String> {
    let Some(frame) = muster_frame::read_frame_or_end(stream, LARGEST_MESSAGE)? else {
        return Ok(None);
    };
    M::decode(frame.as_slice()).map(Some).map_err(|error| {
        format!(
            "a {}-byte frame did not decode as {}: {error}",
            frame.len(),
            std::any::type_name::<M>()
        )
    })
}

/// Why a connection did not open.
#[derive(Debug)]
pub enum HandshakeError {
    /// Nothing answered: no daemon on that socket, or one that hung up.
    Unreachable(String),
    /// A daemon answered and would not talk, for the reason it gave.
    Refused(HelloRefused),
    /// Something answered that did not speak this protocol.
    Garbled(String),
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Unreachable(why) => write!(formatter, "no daemon answered: {why}"),
            HandshakeError::Refused(refused) => write!(
                formatter,
                "the daemon (protocol {}) refused this client (protocol {PROTOCOL}): {}",
                refused.daemon.unwrap_or_default(),
                refused.reason
            ),
            HandshakeError::Garbled(why) => {
                write!(formatter, "what answered does not speak the daemon's protocol: {why}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Dials a daemon's socket and opens a connection of `kind`.
///
/// `client` says who is asking, for the daemon's log.
pub fn connect(
    socket: &Path,
    kind: ConnectionKind,
    client: &str,
) -> Result<(UnixStream, Welcome), HandshakeError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        HandshakeError::Unreachable(format!("could not connect to {}: {error}", socket.display()))
    })?;
    let welcome = open(&mut stream, kind, client)?;
    Ok((stream, welcome))
}

/// The handshake, on a stream already connected.
pub fn open(
    stream: &mut (impl Read + Write),
    kind: ConnectionKind,
    client: &str,
) -> Result<Welcome, HandshakeError> {
    let hello = Hello { protocol: Some(PROTOCOL), kind: kind.into(), client: client.to_string() };
    send(stream, &hello).map_err(|error| HandshakeError::Unreachable(error.to_string()))?;
    let answer = match receive::<crate::HelloAnswer>(stream) {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            return Err(HandshakeError::Unreachable(
                "the daemon hung up before answering the handshake".to_string(),
            ));
        }
        Err(error) => return Err(HandshakeError::Garbled(error)),
    };
    match answer.answer {
        Some(hello_answer::Answer::Welcome(welcome)) => Ok(welcome),
        Some(hello_answer::Answer::Refused(refused)) => Err(HandshakeError::Refused(refused)),
        None => Err(HandshakeError::Garbled("the handshake's answer was empty".to_string())),
    }
}
