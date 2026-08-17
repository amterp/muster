//! A driver over the first sequential concept in the corpus: a case is a list of steps and
//! the expectation is the ordered trace of everything that went out, across every channel.
//! Cases and their reasoning live in corpus/conformance/pane-input.json.

mod support;

use std::sync::Arc;

use conformance::{CaseError, Conformance, fields, hex, strings};
use muster_core::input::{
    Key, KeyAction, KeyEvent, Modifiers, PaneChannel, PaneInput, PaneInputSettings, PaneIntent,
    ScrollDirection,
};
use serde_json::{Value, json};
use support::input::{FakeChannel, FakeEncoder, SendRecorder};

#[test]
fn pane_input_conformance() {
    let corpus = Conformance::load("pane-input.json");

    let ran = corpus.run(|given| {
        let recorder = Arc::new(SendRecorder::default());
        let control: Arc<dyn PaneChannel> =
            Arc::new(FakeChannel::new("control", recorder.clone(), false, true));
        let pane = PaneInput::new(
            control,
            server_channel(given.get("daemon"), &recorder),
            Arc::new(FakeEncoder),
            &PaneInputSettings::default(),
        );

        for step in given.get("steps").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            apply(step, &pane)?;
        }

        let trace: Vec<Value> =
            recorder.sends().iter().map(|(channel, intent)| describe(channel, intent)).collect();
        Ok(fields([("trace", Some(Value::Array(trace)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// The daemon channel a case asks for, or none.
///
/// Absent means no daemon at all - the degraded arrangement where the app has to guess.
/// `refuses` means reachable but declining, which is the wedged-daemon state and a
/// different path from absence.
fn server_channel(
    given: Option<&Value>,
    recorder: &Arc<SendRecorder>,
) -> Option<Arc<dyn PaneChannel>> {
    let given = given?;
    let refuses = given.get("refuses").and_then(Value::as_bool).unwrap_or(false);
    Some(Arc::new(FakeChannel::new("daemon", recorder.clone(), true, !refuses)))
}

fn apply(step: &Value, pane: &PaneInput) -> Result<(), CaseError> {
    if let Some(send) = step.get("send") {
        let name = send
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`send.key` is missing"))?;
        let key = Key::parse(name)
            .ok_or_else(|| CaseError::new(format!("`{name}` is not a W3C key name")))?;
        let modifiers = Modifiers::parse(&strings(send, "modifiers")).ok_or_else(|| {
            CaseError::new("`send.modifiers` names something that is not a modifier")
        })?;
        pane.send(&KeyEvent {
            action: KeyAction::Press,
            key,
            modifiers,
            text: send.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            ..KeyEvent::default()
        });
        return Ok(());
    }
    if let Some(text) = step.get("paste").and_then(Value::as_str) {
        pane.paste(text);
        return Ok(());
    }
    if let Some(scroll) = step.get("scroll") {
        let direction = scroll
            .get("direction")
            .and_then(Value::as_str)
            .and_then(ScrollDirection::parse)
            .ok_or_else(|| CaseError::new("`scroll` needs a known `direction`"))?;
        let lines = scroll
            .get("lines")
            .and_then(Value::as_u64)
            .and_then(|lines| u16::try_from(lines).ok())
            .ok_or_else(|| CaseError::new("`scroll` needs `lines`"))?;
        pane.scroll(direction, lines);
        return Ok(());
    }
    Err(CaseError::new("a step must be one of send, paste, scroll"))
}

fn describe(channel: &str, intent: &PaneIntent) -> Value {
    match intent {
        PaneIntent::Input(bytes) => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("input"))),
            ("bytes_hex", Some(json!(hex(bytes)))),
        ]),
        PaneIntent::Text(text) => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("text"))),
            ("text", Some(json!(text))),
        ]),
        PaneIntent::Key { name } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("key"))),
            ("name", Some(json!(name))),
        ]),
        PaneIntent::Scroll { direction, lines } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("scroll"))),
            ("direction", Some(json!(direction.as_str()))),
            ("lines", Some(json!(lines))),
        ]),
        PaneIntent::Resize { columns, rows } => fields([
            ("channel", Some(json!(channel))),
            ("intent", Some(json!("resize"))),
            ("columns", Some(json!(columns))),
            ("rows", Some(json!(rows))),
        ]),
    }
}
