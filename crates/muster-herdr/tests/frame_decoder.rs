//! The only decidable logic on an otherwise I/O-shaped path, and the one place a pane's
//! whole stream can be lost. Cases live in corpus/conformance/frame-decoder.json.

use conformance::{CaseError, Conformance, fields, hex};
use muster_herdr::{FrameDecoder, PaneStreamEvent};
use serde_json::{Value, json};

#[test]
fn frame_decoder_conformance() {
    let corpus = Conformance::load("frame-decoder.json");

    let ran = corpus.run(|given| {
        let mut decoder = FrameDecoder::new();
        let mut events = Vec::new();
        // `split: "bytes"` re-splits every chunk into single bytes first, which is the worst
        // case a 35 KB repaint can hit and the one that catches a decoder trusting read
        // boundaries.
        let by_byte = given.get("split").and_then(Value::as_str) == Some("bytes");

        let chunks = given
            .get("chunks")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("`chunks` is missing: there is no stream to decode"))?;
        for chunk in chunks {
            let text = chunk
                .as_str()
                .ok_or_else(|| CaseError::new("every chunk must be a string"))?
                .as_bytes();
            if by_byte {
                for byte in text {
                    events.extend(decoder.consume(&[*byte]));
                }
            } else {
                events.extend(decoder.consume(text));
            }
        }

        Ok(fields([("events", Some(Value::Array(events.iter().map(describe).collect())))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn describe(event: &PaneStreamEvent) -> Value {
    match event {
        PaneStreamEvent::Frame(frame) => fields([
            ("kind", Some(json!("frame"))),
            ("bytes_hex", Some(json!(hex(&frame.bytes)))),
            ("full", Some(json!(frame.is_full))),
            ("seq", Some(json!(frame.sequence))),
        ]),
        // Absent rather than null when herdr did not say why: the corpus states what a case
        // is about, and a reason nobody gave is not a reason that is empty.
        PaneStreamEvent::Closed { reason } => fields([
            ("kind", Some(json!("closed"))),
            ("reason", reason.as_ref().map(|reason| json!(reason))),
        ]),
    }
}
