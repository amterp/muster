//! Carrying the API's messages, and the client's half of opening a connection.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use prost::Message;

use crate::version::API;
use crate::{Client, Hello, HelloAnswer, Kind, Refused, Welcome, hello_answer};

/// The most a message either way may be. A frame announcing more is refused before anything is
/// allocated for it.
pub const LARGEST_MESSAGE: u32 = 16 << 20;

/// Writes one message as a frame. One past [`LARGEST_MESSAGE`] is refused here rather than
/// written, since the reader would refuse it and end the connection.
pub fn send(stream: &mut impl Write, message: &impl Message) -> std::io::Result<()> {
    let bytes = message.encode_to_vec();
    if bytes.len() > LARGEST_MESSAGE as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "a message of {} bytes is past the {LARGEST_MESSAGE}-byte ceiling",
                bytes.len()
            ),
        ));
    }
    muster_frame::write_frame(stream, &bytes)
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

/// The Hello a client of this build sends.
pub fn hello(client: Client) -> Hello {
    Hello { kind: Kind::Api.into(), api: Some(API), client: Some(client) }
}

/// Why a connection did not open.
#[derive(Debug)]
pub enum HandshakeError {
    /// Nothing answered: no daemon on that socket, or one that hung up.
    Unreachable(String),
    /// A daemon answered and would not serve this client, for the reason it gave.
    Refused(Refused),
    /// Something answered that did not speak this API.
    Garbled(String),
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Unreachable(why) => write!(formatter, "no daemon answered: {why}"),
            HandshakeError::Refused(refused) => match &refused.api {
                Some(api) => write!(
                    formatter,
                    "the daemon (API {api}) refused this client (API {API}): {}",
                    refused.reason
                ),
                None => write!(
                    formatter,
                    "the daemon refused this client (API {API}), and names no API version, so it \
                     predates the API: {}",
                    refused.reason
                ),
            },
            HandshakeError::Garbled(why) => {
                write!(formatter, "what answered does not speak Muster's API: {why}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Dials a daemon's socket and opens an API connection as `client`.
pub fn connect(socket: &Path, client: Client) -> Result<(UnixStream, Welcome), HandshakeError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        HandshakeError::Unreachable(format!("could not connect to {}: {error}", socket.display()))
    })?;
    let welcome = open(&mut stream, client)?;
    Ok((stream, welcome))
}

/// The handshake, on a stream already connected.
pub fn open(stream: &mut (impl Read + Write), client: Client) -> Result<Welcome, HandshakeError> {
    send(stream, &hello(client)).map_err(|error| HandshakeError::Unreachable(error.to_string()))?;
    answered(stream)
}

/// Reads the daemon's answer to a Hello already sent.
pub fn answered(stream: &mut impl Read) -> Result<Welcome, HandshakeError> {
    match receive::<HelloAnswer>(stream) {
        Ok(Some(HelloAnswer { answer: Some(hello_answer::Answer::Welcome(welcome)) })) => {
            Ok(welcome)
        }
        Ok(Some(HelloAnswer { answer: Some(hello_answer::Answer::Refused(refused)) })) => {
            Err(HandshakeError::Refused(refused))
        }
        Ok(Some(HelloAnswer { answer: None })) => {
            Err(HandshakeError::Garbled("the handshake's answer was empty".to_string()))
        }
        Ok(None) => Err(HandshakeError::Unreachable(
            "the daemon hung up before answering the handshake".to_string(),
        )),
        Err(error) => Err(HandshakeError::Garbled(error)),
    }
}
