//! The encoder is libghostty-vt's, so these do not test escape-sequence generation -
//! upstream does that, and a second implementation written to agree with it would be the
//! bug. What they pin is everything around it that is ours.
//!
//! Cases live in corpus/conformance/key-encoder.json.

use conformance::{CaseError, Conformance, fields, hex, strings};
use muster_core::input::{Key, KeyEvent, Modifiers, OptionAsAlt, TerminalModeProfile};
use muster_vt::KeyEncoder;
use serde_json::{Value, json};

#[test]
fn key_encoder_conformance() {
    let corpus = Conformance::load("key-encoder.json");

    let ran = corpus.run(|given| {
        let encoder = KeyEncoder::new(profile(given.get("profile"))?)
            .map_err(|error| CaseError::new(error.to_string()))?;
        let bytes =
            encoder.encode(&key_event(given)?).map_err(|e| CaseError::new(e.to_string()))?;
        Ok(fields([("bytes_hex", Some(json!(hex(&bytes))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A named profile, or the conservative default with named fields overridden.
///
/// Overrides rather than a whole profile per case: what each case is about is the one
/// setting it changes, and spelling out all seven fields would bury it.
fn profile(given: Option<&Value>) -> Result<TerminalModeProfile, CaseError> {
    let mut profile = TerminalModeProfile::UNKNOWN_PANE;
    match given {
        None => Ok(profile),
        Some(Value::String(name)) => match name.as_str() {
            "unknownPane" => Ok(TerminalModeProfile::UNKNOWN_PANE),
            "herdrTUI" => Ok(TerminalModeProfile::HERDR_TUI),
            other => Err(CaseError::new(format!("`{other}` is not a named profile"))),
        },
        Some(Value::Object(overrides)) => {
            for (name, value) in overrides {
                match name.as_str() {
                    "kittyFlags" => {
                        profile.kitty_flags = u8::try_from(value.as_u64().unwrap_or(0))
                            .map_err(|_| CaseError::new("`kittyFlags` does not fit in a byte"))?;
                    }
                    "applicationCursorKeys" => {
                        profile.application_cursor_keys = value.as_bool().unwrap_or(false);
                    }
                    "applicationKeypad" => {
                        profile.application_keypad = value.as_bool().unwrap_or(false);
                    }
                    "altSendsEscapePrefix" => {
                        profile.alt_sends_escape_prefix = value.as_bool().unwrap_or(false);
                    }
                    "modifyOtherKeys" => {
                        profile.modify_other_keys = value.as_bool().unwrap_or(false);
                    }
                    "bracketedPaste" => profile.bracketed_paste = value.as_bool().unwrap_or(false),
                    "optionActsAsAlt" => {
                        profile.option_acts_as_alt =
                            value.as_str().and_then(OptionAsAlt::parse).ok_or_else(|| {
                                CaseError::new("`optionActsAsAlt` is not one of the four")
                            })?;
                    }
                    other => {
                        return Err(CaseError::new(format!("`{other}` is not a profile setting")));
                    }
                }
            }
            Ok(profile)
        }
        Some(other) => Err(CaseError::new(format!("`profile` is {other}, which names nothing"))),
    }
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
    Ok(KeyEvent {
        key,
        modifiers,
        text: given.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        unshifted_codepoint: given
            .get("unshiftedCodepoint")
            .and_then(Value::as_str)
            .and_then(|text| text.chars().next()),
        is_composing: given.get("isComposing").and_then(Value::as_bool).unwrap_or(false),
        ..KeyEvent::default()
    })
}
