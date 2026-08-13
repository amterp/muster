//! Turns a pane's newline-delimited JSON stream back into frames.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

/// One frame off a pane's data plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneFrame {
    /// The ANSI the daemon rendered for this pane's screen. Already encoded for a terminal,
    /// and already stripped of the inner program's mode changes.
    pub bytes: Vec<u8>,
    /// Whether this repaints the whole screen rather than a diff against the last one.
    pub is_full: bool,
    /// Monotonic per pane. The data plane has sequence numbers even though the control
    /// plane does not, so staleness here is detectable.
    pub sequence: i64,
}

/// What a decoded line off the stream turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneStreamEvent {
    Frame(PaneFrame),
    /// The daemon hung up on this pane.
    Closed {
        reason: Option<String>,
    },
}

/// Pure, and deliberately so.
///
/// This is the only decidable logic in an otherwise I/O-shaped path, and keeping it apart
/// from the bridge that does the reading is what lets the awkward parts be tested: a 35 KB
/// repaint split across arbitrary reads, a partial line at the end of a chunk, garbage
/// between good frames.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    pending: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> FrameDecoder {
        FrameDecoder::default()
    }

    /// Feeds a chunk of stream and returns whatever completed inside it.
    ///
    /// Anything past the last newline is held: frames routinely arrive split across reads,
    /// and half a JSON object decodes to nothing rather than to something wrong.
    pub fn consume(&mut self, chunk: &[u8]) -> Vec<PaneStreamEvent> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            // `take` keeps the line and drops the newline, and the drain still removes the
            // whole range either way - which is what advances the buffer past this line.
            let line: Vec<u8> = self.pending.drain(..=newline).take(newline).collect();
            if let Some(event) = decode(&line) {
                events.push(event);
            }
        }

        events
    }
}

/// Decodes one line, or nothing.
///
/// A line we cannot read is skipped rather than fatal. herdr's API is explicitly unstable,
/// and an unknown message type is a thing it may add next week - dropping the pane's whole
/// stream over one is a worse failure than ignoring it.
fn decode(line: &[u8]) -> Option<PaneStreamEvent> {
    let object: Value = serde_json::from_slice(line).ok()?;
    match object.get("type")?.as_str()? {
        "terminal.frame" => {
            let bytes = BASE64.decode(object.get("bytes")?.as_str()?).ok()?;
            Some(PaneStreamEvent::Frame(PaneFrame {
                bytes,
                is_full: object.get("full").and_then(Value::as_bool).unwrap_or(false),
                sequence: object.get("seq").and_then(Value::as_i64).unwrap_or(0),
            }))
        }
        "terminal.closed" => Some(PaneStreamEvent::Closed {
            reason: object.get("reason").and_then(Value::as_str).map(str::to_string),
        }),
        _ => None,
    }
}
