//! Reading a keystroke the way the corpus spells one.
//!
//! Shared rather than written twice because two files describe keystrokes in the same
//! vocabulary: the cases in `key-encoder.json`, and the keystroke list its `survey` section
//! carries. Two parsers would let the two mean subtly different things - a `modifiers` that
//! one reads and the other ignores - and the disagreement would surface as a snapshot
//! nobody could account for.

use conformance::{CaseError, strings};
use muster_core::input::{Key, KeyEvent, Modifiers, OptionAsAlt, TerminalModeProfile};
use serde_json::Value;

/// A named profile, or the conservative default with named fields overridden.
///
/// Overrides rather than a whole profile per case: what each case is about is the one
/// setting it changes, and spelling out all seven fields would bury it.
pub(crate) fn profile(given: Option<&Value>) -> Result<TerminalModeProfile, CaseError> {
    let mut profile = TerminalModeProfile::UNKNOWN_PANE;
    match given {
        None => Ok(profile),
        Some(Value::String(name)) => named_profile(name),
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

/// One of the two profiles the corpus names, by name.
pub(crate) fn named_profile(name: &str) -> Result<TerminalModeProfile, CaseError> {
    match name {
        "unknownPane" => Ok(TerminalModeProfile::UNKNOWN_PANE),
        "herdrTUI" => Ok(TerminalModeProfile::HERDR_TUI),
        other => Err(CaseError::new(format!("`{other}` is not a named profile"))),
    }
}

pub(crate) fn key_event(given: &Value) -> Result<KeyEvent, CaseError> {
    let name = given
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("`key` is missing"))?;
    let key = Key::parse(name)
        .ok_or_else(|| CaseError::new(format!("`{name}` is not a W3C key name")))?;
    let modifiers = Modifiers::parse(&strings(given, "modifiers"))
        .ok_or_else(|| CaseError::new("`modifiers` names something that is not a modifier"))?;
    // Which modifiers the layout spent producing the text. Absent means none were, which is
    // what an unmodified press looks like - but a case about option has to say, because
    // whether the encoder may use the text at all turns on this and on nothing else.
    let consumed_modifiers =
        Modifiers::parse(&strings(given, "consumedModifiers")).ok_or_else(|| {
            CaseError::new("`consumedModifiers` names something that is not a modifier")
        })?;
    Ok(KeyEvent {
        key,
        modifiers,
        consumed_modifiers,
        text: given.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        text_without_option: given
            .get("textWithoutOption")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        unshifted_codepoint: given
            .get("unshiftedCodepoint")
            .and_then(Value::as_str)
            .and_then(|text| text.chars().next()),
        is_composing: given.get("isComposing").and_then(Value::as_bool).unwrap_or(false),
        ..KeyEvent::default()
    })
}
