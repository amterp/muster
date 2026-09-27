//! Input precedence: the keymap gets first refusal on every chord, and only what it
//! declines is reported toward the pane. Cases live in corpus/conformance/keymap.json.

use conformance::{CaseError, Conformance, fields, hex, strings};
use muster_core::input::{Key, KeyAction, KeyEvent, Keymap, Modifiers, Resolution};
use serde_json::{Value, json};

#[test]
fn keymap_conformance() {
    let corpus = Conformance::load("keymap.json");
    let keymap = Keymap::default();

    let ran = corpus.run(|given| {
        let event = key_event(given)?;
        Ok(match keymap.resolve(&event) {
            Resolution::Text(bytes) => {
                fields([("kind", Some(json!("text"))), ("bytes_hex", Some(json!(hex(&bytes))))])
            }
            Resolution::ServerEncoded(name) => {
                fields([("kind", Some(json!("serverEncoded"))), ("key", Some(json!(name)))])
            }
            Resolution::Action(_) => fields([("kind", Some(json!("action")))]),
            Resolution::Unbound => fields([("kind", Some(json!("unbound")))]),
        })
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn key_event(given: &Value) -> Result<KeyEvent, CaseError> {
    let name = given
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("`key` is missing"))?;
    let key = Key::parse(name)
        .ok_or_else(|| CaseError::new(format!("`{name}` is not a W3C key name")))?;
    let modifiers = Modifiers::parse(&strings(given, "modifiers"))
        .ok_or_else(|| CaseError::new("`modifiers` names something that is not a modifier"))?;
    let action = match given.get("action").and_then(Value::as_str) {
        None => KeyAction::Press,
        Some(name) => KeyAction::parse(name)
            .ok_or_else(|| CaseError::new(format!("`{name}` is not a key action")))?,
    };
    Ok(KeyEvent { action, key, modifiers, ..KeyEvent::default() })
}
