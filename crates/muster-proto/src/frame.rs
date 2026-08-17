//! How a message is carried over a socket, in the one place both ends read it from.
//!
//! A four-byte big-endian length, then that many bytes of protobuf. One request and one answer
//! per connection, so nothing here needs a message type or a sequence number - which end is
//! talking is decided by who dialed.
//!
//! In this crate rather than beside the endpoint because the two ends are separate programs.
//! The CLI is built from this repo but runs as whatever version somebody has on their PATH, so
//! the framing has to be something both sides agree on without negotiating, and two copies of
//! it would be two chances to disagree.

use std::io::{Read, Write};

/// The most a message either way may be.
///
/// Every request Muster has is a few hundred bytes and the largest imaginable is a paste; the
/// largest answer is a `Window` for a window nobody can fill past about fifteen panes. This is
/// here so that a caller who is not Muster's CLI - a port scanner, a truncated write, a client
/// built against a different schema - cannot make the app reserve a gigabyte by claiming to be
/// about to send one, and so that the CLI is protected the same way from the same mistake.
pub const LARGEST_MESSAGE: u32 = 1 << 20;

/// Reads a four-byte big-endian length, then that many bytes.
///
/// Big-endian because that is what a wire length is everywhere it is not being read by the
/// machine that wrote it.
///
/// `most` is the caller's own idea of what is too big to be worth reading.
pub fn read_frame(stream: &mut impl Read, most: u32) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).map_err(|error| error.to_string())?;
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
    Ok(payload)
}

/// Writes a length and a payload, as [`read_frame`] expects them.
pub fn write_frame(stream: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}
