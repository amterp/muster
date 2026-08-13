//! What Muster can say on a pane's control stream.
//!
//! herdr's control stream takes four commands and this covers the three Muster sends
//! (`terminal.release` is a detach, which closing the stream already does). The set is
//! small because herdr's is: everything a client can express about input on this channel is
//! here.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use muster_core::input::ScrollDirection;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlStreamMessage {
    /// Raw bytes for the pane's PTY.
    ///
    /// Raw is not a shortcut: herdr writes these to the PTY untouched, so whatever is here
    /// is exactly what the program receives (`docs/observations/herdr-0.8.0.md` section 5).
    Input(Vec<u8>),

    /// The pane's new grid size, in cells.
    Resize { columns: u16, rows: u16 },

    /// A scroll, as an intent rather than as bytes.
    ///
    /// The daemon answers this against the pane's real modes - encoding a wheel event for a
    /// mouse-reporting program, sending alternate-scroll keys, or moving its own scrollback.
    /// It is the one input-shaped thing Muster does not have to guess about.
    Scroll { direction: ScrollDirection, lines: u16 },
}

impl ControlStreamMessage {
    /// The message as herdr's newline-delimited JSON.
    ///
    /// The key names are herdr's and are written out literally, so they read against their
    /// source rather than hiding behind a derive. herdr parses these with serde, so a wrong
    /// name is a silently ignored command rather than an error anyone would see - a pane
    /// that renders perfectly and never receives a keystroke.
    ///
    /// The trailing newline is not decoration: herdr reads its stdin as newline-delimited
    /// JSON and blocks without one.
    pub fn wire_format(&self) -> Vec<u8> {
        let object = match self {
            ControlStreamMessage::Input(bytes) => {
                json!({ "type": "terminal.input", "bytes": BASE64.encode(bytes) })
            }
            ControlStreamMessage::Resize { columns, rows } => {
                json!({ "type": "terminal.resize", "cols": columns, "rows": rows })
            }
            ControlStreamMessage::Scroll { direction, lines } => json!({
                "type": "terminal.scroll",
                "direction": direction.as_str(),
                "lines": lines,
            }),
        };
        let mut out = object.to_string().into_bytes();
        out.push(b'\n');
        out
    }
}
