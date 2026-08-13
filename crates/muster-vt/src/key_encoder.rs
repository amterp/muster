//! Turns a keystroke into the bytes a terminal program expects.
//!
//! This is libghostty-vt's own key encoder, which matters more than it sounds: it is the
//! same code the pane's terminal would have used if the pane's terminal were doing the
//! encoding, and the same code herdr's TUI path runs (`src/ghostty/mod.rs:2552`). Muster is
//! not writing a second implementation that has to agree with a first one.
//!
//! What it cannot supply is the state to encode against. See `TerminalModeProfile`.

use std::ffi::c_void;
use std::fmt;

use muster_core::input::{
    EncodeError, KeyAction, KeyEncoding, KeyEvent, OptionAsAlt, TerminalModeProfile,
};

use crate::ffi;
use crate::key_mapping::ghostty_key;

/// 128 covers every sequence the protocol can produce for an ordinary keystroke. The retry
/// past it is not dead code: with associated-text reporting on, a key can carry arbitrary
/// text.
const FIRST_TRY_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderError {
    CreationFailed(i32),
    EncodingFailed(i32),
}

impl fmt::Display for EncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncoderError::CreationFailed(code) => {
                write!(f, "libghostty-vt would not create a key encoder (result {code})")
            }
            EncoderError::EncodingFailed(code) => {
                write!(f, "libghostty-vt could not encode this keystroke (result {code})")
            }
        }
    }
}

impl std::error::Error for EncoderError {}

/// An encoder fixed to one set of pane modes.
///
/// Fixed rather than per-call because a pane's modes change rarely and a keystroke happens
/// on every keypress: this is on the input-to-glyph path the perf budget is written
/// against, so the per-key work is one struct fill and one encode.
#[derive(Debug)]
pub struct KeyEncoder {
    encoder: ffi::GhosttyKeyEncoder,
    event: ffi::GhosttyKeyEvent,
}

// SAFETY: the two handles are owned by this value and reached only through `&mut self`, so
// libghostty-vt never sees two callers at once. herdr holds the same handles under external
// synchronization for the same reason (`src/ghostty/mod.rs:2557`).
unsafe impl Send for KeyEncoder {}

impl KeyEncoder {
    pub fn new(profile: TerminalModeProfile) -> Result<KeyEncoder, EncoderError> {
        let mut encoder: ffi::GhosttyKeyEncoder = std::ptr::null_mut();
        // SAFETY: a null allocator asks for libghostty's default, and the out parameter is
        // a handle we own.
        let created = unsafe { ffi::ghostty_key_encoder_new(std::ptr::null(), &raw mut encoder) };
        if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || encoder.is_null() {
            return Err(EncoderError::CreationFailed(created));
        }

        // The event is reused across calls for the same reason the encoder is.
        let mut event: ffi::GhosttyKeyEvent = std::ptr::null_mut();
        // SAFETY: as above.
        let created = unsafe { ffi::ghostty_key_event_new(std::ptr::null(), &raw mut event) };
        if created != ffi::GhosttyResult_GHOSTTY_SUCCESS || event.is_null() {
            // SAFETY: the encoder was created above and is freed exactly once here.
            unsafe { ffi::ghostty_key_encoder_free(encoder) };
            return Err(EncoderError::CreationFailed(created));
        }

        let encoder = KeyEncoder { encoder, event };
        encoder.apply(profile);
        Ok(encoder)
    }

    fn apply(&self, profile: TerminalModeProfile) {
        let mut kitty_flags = profile.kitty_flags;
        let mut cursor_keys = profile.application_cursor_keys;
        let mut keypad = profile.application_keypad;
        let mut alt_escape = profile.alt_sends_escape_prefix;
        let mut modify_other_keys = profile.modify_other_keys;
        let mut option_as_alt = ghostty_option_as_alt(profile.option_acts_as_alt);

        // SAFETY: each option's pointer is to a local of the type libghostty documents for
        // that option, and the call copies it. Getting one of these types wrong is the real
        // hazard here rather than the pointers: passing a bool where the enum is expected
        // reads four bytes from one, and the encoder then sees whatever followed it.
        unsafe {
            let set = |option, value: *mut c_void| {
                ffi::ghostty_key_encoder_setopt(self.encoder, option, value);
            };
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS,
                (&raw mut kitty_flags).cast(),
            );
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION,
                (&raw mut cursor_keys).cast(),
            );
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION,
                (&raw mut keypad).cast(),
            );
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_ALT_ESC_PREFIX,
                (&raw mut alt_escape).cast(),
            );
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MODIFY_OTHER_KEYS_STATE_2,
                (&raw mut modify_other_keys).cast(),
            );
            set(
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw mut option_as_alt).cast(),
            );
        }
    }

    /// The bytes this keystroke should put on the pane's input.
    ///
    /// Empty means the key produces nothing - a bare modifier press under a profile that
    /// does not report them, or a keystroke the input method has claimed. Empty is a normal
    /// answer, not a failure, and callers must not send anything for it.
    pub fn encode(&self, key: &KeyEvent) -> Result<Vec<u8>, EncoderError> {
        // A composing keystroke belongs to the input method. Encoding it would deliver the
        // romaji as well as the characters it composes into.
        if key.is_composing {
            return Ok(Vec::new());
        }

        let text = key.text.as_bytes();
        // SAFETY: every setter takes the event handle this value owns, and `set_utf8`
        // borrows `text` only for the duration of the call.
        unsafe {
            ffi::ghostty_key_event_set_action(self.event, ghostty_action(key.action));
            ffi::ghostty_key_event_set_key(self.event, ghostty_key(key.key));
            ffi::ghostty_key_event_set_mods(self.event, key.modifiers.0);
            ffi::ghostty_key_event_set_consumed_mods(self.event, key.consumed_modifiers.0);
            ffi::ghostty_key_event_set_composing(self.event, false);
            ffi::ghostty_key_event_set_unshifted_codepoint(
                self.event,
                key.unshifted_codepoint.map_or(0, |c| c as u32),
            );
            ffi::ghostty_key_event_set_utf8(self.event, text.as_ptr().cast(), text.len());
        }

        let mut buffer = vec![0u8; FIRST_TRY_BYTES];
        let mut length = 0usize;
        // SAFETY: the buffer is ours and its length is reported honestly; on
        // GHOSTTY_OUT_OF_SPACE libghostty writes the size it needs into `length` instead.
        let mut result = unsafe { self.encode_into(&mut buffer, &raw mut length) };
        if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            buffer = vec![0u8; length];
            // SAFETY: as above, now with the capacity libghostty asked for.
            result = unsafe { self.encode_into(&mut buffer, &raw mut length) };
        }

        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return Err(EncoderError::EncodingFailed(result));
        }
        buffer.truncate(length);
        Ok(buffer)
    }

    unsafe fn encode_into(&self, buffer: &mut [u8], length: *mut usize) -> ffi::GhosttyResult {
        // SAFETY: the caller guarantees `length` points at a usize it owns; the buffer is a
        // live slice for the duration of the call.
        unsafe {
            ffi::ghostty_key_encoder_encode(
                self.encoder,
                self.event,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                length,
            )
        }
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        // SAFETY: both handles were created by `new` and are freed exactly once.
        unsafe {
            ffi::ghostty_key_event_free(self.event);
            ffi::ghostty_key_encoder_free(self.encoder);
        }
    }
}

/// The encoder the core asks for.
///
/// Implemented here rather than on the trait's own side because the dependency runs this
/// way: the core must not know libghostty-vt exists.
impl KeyEncoding for KeyEncoder {
    fn encode(&self, key: &KeyEvent) -> Result<Vec<u8>, EncodeError> {
        KeyEncoder::encode(self, key).map_err(|error| EncodeError(error.to_string()))
    }
}

// SAFETY: `KeyEncoding` requires Sync, and the handles are only reachable through this
// type's own methods. PaneInput serializes every send through one lock, which is the
// external synchronization libghostty's encoder expects.
unsafe impl Sync for KeyEncoder {}

fn ghostty_action(action: KeyAction) -> ffi::GhosttyKeyAction {
    match action {
        KeyAction::Press => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS,
        KeyAction::Release => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_RELEASE,
        KeyAction::Repeated => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_REPEAT,
    }
}

fn ghostty_option_as_alt(option: OptionAsAlt) -> ffi::GhosttyOptionAsAlt {
    match option {
        OptionAsAlt::Never => ffi::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_FALSE,
        OptionAsAlt::Always => ffi::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_TRUE,
        OptionAsAlt::LeftOnly => ffi::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_LEFT,
        OptionAsAlt::RightOnly => ffi::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_RIGHT,
    }
}
