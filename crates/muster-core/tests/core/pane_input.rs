//! A driver over what one pane's input path sends, in order. Cases and their reasoning live in
//! corpus/conformance/pane-input.json.

use std::sync::Arc;

use crate::support::input::RecordingSink;
use conformance::{CaseError, Conformance, fields, hex, strings};
use muster_core::input::{
    InputEvent, Key, KeyAction, KeyEvent, Modifiers, OptionAsAlt, PaneInput, PaneInputSettings,
};
use muster_core::mirror::backend::PaneId;
use serde_json::{Value, json};

#[test]
fn pane_input_conformance() {
    let corpus = Conformance::load("pane-input.json");

    let ran = corpus.run(|given| {
        let sink = Arc::new(RecordingSink::default());
        let option_as_alt = match given.get("optionAsAlt").and_then(Value::as_str) {
            Some(name) => OptionAsAlt::parse(name)
                .ok_or_else(|| CaseError::new(format!("`{name}` is not an option-as-alt")))?,
            None => OptionAsAlt::Never,
        };
        let settings = PaneInputSettings { option_as_alt, ..PaneInputSettings::default() };
        let pane = PaneInput::new(PaneId::new("p1"), sink.clone(), &settings);

        for step in given.get("steps").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            apply(step, &pane)?;
        }

        let trace: Vec<Value> = sink.sent().iter().map(|(_, event)| describe(event)).collect();
        Ok(fields([("trace", Some(Value::Array(trace)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn apply(step: &Value, pane: &PaneInput) -> Result<(), CaseError> {
    if let Some(send) = step.get("send") {
        let name = send
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`send.key` is missing"))?;
        let key = Key::parse(name)
            .ok_or_else(|| CaseError::new(format!("`{name}` is not a W3C key name")))?;
        let modifiers = |field: &str| {
            Modifiers::parse(&strings(send, field)).ok_or_else(|| {
                CaseError::new(format!("`send.{field}` names something that is not a modifier"))
            })
        };
        let text = |field: &str| send.get(field).and_then(Value::as_str).unwrap_or("").to_string();
        pane.send(&KeyEvent {
            action: KeyAction::Press,
            key,
            modifiers: modifiers("modifiers")?,
            consumed_modifiers: modifiers("consumedModifiers")?,
            text: text("text"),
            text_without_option: text("textWithoutOption"),
            ..KeyEvent::default()
        });
        return Ok(());
    }
    if let Some(paste) = step.get("paste").and_then(Value::as_str) {
        pane.paste(paste, step.get("confirmed").and_then(Value::as_bool).unwrap_or(false));
        return Ok(());
    }
    if let Some(text) = step.get("text").and_then(Value::as_str) {
        pane.send_text(text);
        return Ok(());
    }
    Err(CaseError::new(format!("a step that is not a send, a paste or a text: {step}")))
}

fn describe(event: &InputEvent) -> Value {
    match event {
        InputEvent::Key { key, option_as_alt } => json!({
            "event": "key",
            "key": key.key.as_str(),
            "text": key.text,
            "optionAsAlt": option_as_alt.as_str(),
        }),
        InputEvent::Bytes(bytes) => json!({ "event": "bytes", "bytes_hex": hex(bytes) }),
        InputEvent::Paste { text, confirmed } => {
            json!({ "event": "paste", "text": text, "confirmed": confirmed })
        }
        InputEvent::Send { text, enter } => {
            json!({ "event": "send", "text": text, "enter": enter })
        }
        InputEvent::Wheel(wheel) => json!({ "event": "wheel", "dx": wheel.dx, "dy": wheel.dy }),
        InputEvent::Mouse(mouse) => {
            json!({ "event": "mouse", "action": format!("{:?}", mouse.action) })
        }
        InputEvent::Focus(focused) => json!({ "event": "focus", "focused": focused }),
        InputEvent::ClearScreen { key, .. } => {
            json!({ "event": "clear_screen", "key": key.as_ref().map(|key| key.key.as_str()) })
        }
        InputEvent::Reset => json!({ "event": "reset" }),
    }
}
