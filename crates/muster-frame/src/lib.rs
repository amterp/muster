//! How a protobuf message is carried over a socket: a four-byte big-endian length, then that
//! many bytes.
//!
//! Two protocols use it. The command socket between the CLI and a window (`muster-proto`)
//! sends one request per connection, and the daemon's protocol (`muster-daemon-proto`) keeps
//! a connection open for as long as a client holds it. A crate of its own so that neither
//! protocol owns the other's framing, and so that there is one copy of it for two separately
//! built programs to agree on rather than two chances to disagree.
//!
//! Nothing here knows a message type or a sequence number. Each protocol decides what a frame
//! holds and how large one may be.

use std::io::{ErrorKind, Read, Write};

/// Reads a four-byte big-endian length, then that many bytes.
///
/// Big-endian because that is what a wire length is everywhere it is not being read by the
/// machine that wrote it.
///
/// `most` is the caller's own idea of what is too big to be worth reading. A length over it is
/// refused before a byte of the payload is allocated, so a peer that is not what it claims to
/// be cannot make this side reserve a gigabyte by announcing one.
pub fn read_frame(stream: &mut impl Read, most: u32) -> Result<Vec<u8>, String> {
    match read_frame_or_end(stream, most)? {
        Some(payload) => Ok(payload),
        None => Err("the other end hung up before sending anything".to_string()),
    }
}

/// The same, for a connection that carries many frames: `None` when the other end hung up
/// between two of them.
///
/// A peer closing a persistent connection is how it says it is finished, and that is a
/// different thing from a frame cut off half way. Only the first is `None`; a hang-up inside
/// a length or a payload is still an error, because what arrived is not a message.
pub fn read_frame_or_end(stream: &mut impl Read, most: u32) -> Result<Option<Vec<u8>>, String> {
    let mut length = [0u8; 4];
    let mut filled = 0;
    while filled < length.len() {
        match stream.read(&mut length[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(format!(
                    "the other end hung up {filled} bytes into a frame's length, so what it \
                     sent last is not a whole message"
                ));
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let length = u32::from_be_bytes(length);
    if length > most {
        return Err(format!(
            "the other end said it was about to send {length} bytes, and {most} is as much as \
             this side will read. Refused without reading, so either this is not a Muster \
             client or the two ends were built against schemas that disagree."
        ));
    }
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload).map_err(|error| error.to_string())?;
    Ok(Some(payload))
}

/// Writes a length and a payload, as [`read_frame`] expects them.
pub fn write_frame(stream: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payloads: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for payload in payloads {
            write_frame(&mut bytes, payload).unwrap();
        }
        bytes
    }

    #[test]
    fn frames_come_back_in_order_and_then_the_end() {
        let bytes = framed(&[b"one", b"", b"three"]);
        let mut reader = bytes.as_slice();
        assert_eq!(read_frame_or_end(&mut reader, 16).unwrap().as_deref(), Some(&b"one"[..]));
        assert_eq!(read_frame_or_end(&mut reader, 16).unwrap().as_deref(), Some(&b""[..]));
        assert_eq!(read_frame_or_end(&mut reader, 16).unwrap().as_deref(), Some(&b"three"[..]));
        assert_eq!(read_frame_or_end(&mut reader, 16).unwrap(), None);
    }

    #[test]
    fn a_hang_up_inside_a_frame_is_an_error_rather_than_the_end() {
        let bytes = framed(&[b"payload"]);
        for cut in 1..bytes.len() {
            let mut reader = &bytes[..cut];
            assert!(read_frame_or_end(&mut reader, 16).is_err(), "cut at {cut} read as a frame");
        }
    }

    #[test]
    fn a_length_over_the_ceiling_is_refused_unread() {
        let bytes = framed(&[&[0u8; 17]]);
        let error = read_frame_or_end(&mut bytes.as_slice(), 16).unwrap_err();
        assert!(error.contains("17 bytes"), "{error}");
    }

    #[test]
    fn read_frame_calls_an_immediate_hang_up_an_error() {
        assert!(read_frame(&mut &b""[..], 16).is_err());
    }
}
