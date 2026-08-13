//! The daemon-facing oracle: the exact bytes a message puts on the wire. Cases live in
//! corpus/conformance/control-stream-message.json.

use conformance::{CaseError, Conformance};
use muster_core::input::ScrollDirection;
use muster_herdr::ControlStreamMessage;
use serde_json::{Value, json};

#[test]
fn control_stream_conformance() {
    let corpus = Conformance::load("control-stream-message.json");

    let ran = corpus.run(|given| {
        let wire = message(given)?.wire_format();

        // The trailing newline is pinned by every case rather than assumed, because herdr
        // reads its stdin as newline-delimited JSON and blocks without one - a failure that
        // looks like a pane ignoring the keyboard.
        let newline_terminated = wire.last() == Some(&b'\n');
        let object: Value = serde_json::from_slice(&wire[..wire.len().saturating_sub(1)])
            .map_err(|e| CaseError::new(format!("the message is not JSON: {e}")))?;

        let mut described = object;
        described["newline_terminated"] = json!(newline_terminated);
        Ok(described)
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn message(given: &Value) -> Result<ControlStreamMessage, CaseError> {
    match given.get("intent").and_then(Value::as_str) {
        Some("input") => {
            let hex = given
                .get("bytes_hex")
                .and_then(Value::as_str)
                .ok_or_else(|| CaseError::new("`input` needs `bytes_hex`"))?;
            Ok(ControlStreamMessage::Input(unhex(hex)?))
        }
        Some("resize") => Ok(ControlStreamMessage::Resize {
            columns: number(given, "columns")?,
            rows: number(given, "rows")?,
        }),
        Some("scroll") => {
            let direction = given
                .get("direction")
                .and_then(Value::as_str)
                .and_then(ScrollDirection::parse)
                .ok_or_else(|| CaseError::new("`scroll` needs a known `direction`"))?;
            Ok(ControlStreamMessage::Scroll { direction, lines: number(given, "lines")? })
        }
        other => Err(CaseError::new(format!("`intent` is {other:?}, which is not a message"))),
    }
}

fn number(given: &Value, key: &str) -> Result<u16, CaseError> {
    given
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| CaseError::new(format!("`{key}` is missing or out of range")))
}

fn unhex(text: &str) -> Result<Vec<u8>, CaseError> {
    if !text.len().is_multiple_of(2) {
        return Err(CaseError::new(format!("`{text}` is not a whole number of bytes")));
    }
    (0..text.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&text[at..at + 2], 16)
                .map_err(|_| CaseError::new(format!("`{text}` is not hex")))
        })
        .collect()
}
