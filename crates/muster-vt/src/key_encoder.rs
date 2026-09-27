//! Turns a keystroke into the bytes a terminal program expects.
//!
//! This is libghostty-vt's own key encoder, which matters more than it sounds: it is the
//! same code the pane's terminal would have used if the pane's terminal were doing the
//! encoding. Muster is not writing a second implementation that has to agree with a first one.
//!
//! The modes to encode against come from the pane's own terminal (`configure_from`), which the
//! daemon holds. [`KeyModes`] states them outright, for an encoder with no terminal behind it.

use std::ffi::c_void;
use std::fmt;

use muster_core::input::{Key, KeyAction, KeyEvent, OptionAsAlt};

use crate::ffi;
use crate::key_mapping::ghostty_key;
use crate::terminal::Terminal;

/// 128 covers every sequence the protocol can produce for an ordinary keystroke. The retry
/// past it is not dead code: with associated-text reporting on, a key can carry arbitrary
/// text.
const FIRST_TRY_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderError {
    CreationFailed(i32),
    EncodingFailed(i32),
    UnknownKey(u32),
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
            EncoderError::UnknownKey(code) => {
                write!(f, "{code} is not a key this libghostty-vt knows")
            }
        }
    }
}

impl std::error::Error for EncoderError {}

/// A keystroke as libghostty numbers it: its key code and modifier bits are libghostty's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKeyEvent<'a> {
    pub action: KeyAction,
    pub code: u32,
    pub modifiers: u16,
    pub consumed_modifiers: u16,
    pub text: &'a str,
    /// Zero when there is none.
    pub unshifted_codepoint: u32,
    pub composing: bool,
}

/// Kitty keyboard protocol flag bits.
pub mod kitty_flags {
    pub const DISAMBIGUATE: u8 = 1;
    pub const REPORT_EVENT_TYPES: u8 = 2;
    pub const REPORT_ALTERNATE_KEYS: u8 = 4;
    pub const REPORT_ALL_KEYS_AS_ESCAPE_CODES: u8 = 8;
    pub const REPORT_ASSOCIATED_TEXT: u8 = 16;
}

/// The input modes a keystroke is encoded against, stated rather than read from a terminal.
///
/// The default is a terminal no program has changed: no kitty flags, normal cursor and keypad
/// keys, alt sending an escape prefix, no modifyOtherKeys.
// Five independent modes a program negotiates one at a time; a bitmask would lose the names.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyModes {
    pub kitty_flags: u8,
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
    pub alt_sends_escape_prefix: bool,
    pub modify_other_keys: bool,
    pub option_acts_as_alt: OptionAsAlt,
}

impl Default for KeyModes {
    fn default() -> KeyModes {
        KeyModes {
            kitty_flags: 0,
            application_cursor_keys: false,
            application_keypad: false,
            alt_sends_escape_prefix: true,
            modify_other_keys: false,
            option_acts_as_alt: OptionAsAlt::Never,
        }
    }
}

/// libghostty's key code for a key: what a daemon's input connection carries for a keystroke.
pub fn key_code(key: Key) -> u32 {
    ghostty_key(key)
}

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
    pub fn new(modes: KeyModes) -> Result<KeyEncoder, EncoderError> {
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
        encoder.apply(modes);
        Ok(encoder)
    }

    /// Takes the pane's real input modes from its terminal - cursor and keypad application
    /// modes, alt-escape, modifyOtherKeys and kitty flags - instead of stated ones. Option as
    /// alt is not terminal state, so it is set again from `option_as_alt`.
    pub fn configure_from(&mut self, terminal: &Terminal, option_as_alt: OptionAsAlt) {
        let mut option_as_alt = ghostty_option_as_alt(option_as_alt);
        // SAFETY: both handles are live; libghostty only reads the terminal. It resets
        // option-as-alt, which is why the call after it puts it back.
        unsafe {
            ffi::ghostty_key_encoder_setopt_from_terminal(self.encoder, terminal.handle());
            ffi::ghostty_key_encoder_setopt(
                self.encoder,
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw mut option_as_alt).cast(),
            );
        }
    }

    /// Whether option acts as alt, which is the app's setting rather than the pane's.
    pub fn set_option_as_alt(&mut self, option_as_alt: OptionAsAlt) {
        let mut option_as_alt = ghostty_option_as_alt(option_as_alt);
        // SAFETY: the option reads one GhosttyOptionAsAlt, which libghostty copies.
        unsafe {
            ffi::ghostty_key_encoder_setopt(
                self.encoder,
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw mut option_as_alt).cast(),
            );
        }
    }

    fn apply(&self, modes: KeyModes) {
        let mut kitty_flags = modes.kitty_flags;
        let mut cursor_keys = modes.application_cursor_keys;
        let mut keypad = modes.application_keypad;
        let mut alt_escape = modes.alt_sends_escape_prefix;
        let mut modify_other_keys = modes.modify_other_keys;
        let mut option_as_alt = ghostty_option_as_alt(modes.option_acts_as_alt);

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
        self.encode_raw(&RawKeyEvent {
            action: key.action,
            code: ghostty_key(key.key),
            modifiers: key.modifiers.0,
            consumed_modifiers: key.consumed_modifiers.0,
            text: &key.text,
            unshifted_codepoint: key.unshifted_codepoint.map_or(0, |c| c as u32),
            composing: key.is_composing,
        })
    }

    /// As `encode`, for a key already numbered as libghostty numbers it - which is how a key
    /// arrives from a surface, and so on the daemon's wire.
    pub fn encode_raw(&self, key: &RawKeyEvent<'_>) -> Result<Vec<u8>, EncoderError> {
        // Zig reads the code as an enum, and a value outside it is undefined behavior there
        // rather than an error, so an unknown key never gets that far.
        if key.code > ffi::GhosttyKey_GHOSTTY_KEY_PASTE {
            return Err(EncoderError::UnknownKey(key.code));
        }
        // A composing keystroke belongs to the input method. Encoding it would deliver the
        // romaji as well as the characters it composes into.
        if key.composing {
            return Ok(Vec::new());
        }

        let text = key.text.as_bytes();
        // SAFETY: every setter takes the event handle this value owns, and `set_utf8`
        // borrows `text` only for the duration of the call.
        unsafe {
            ffi::ghostty_key_event_set_action(self.event, ghostty_action(key.action));
            ffi::ghostty_key_event_set_key(self.event, key.code);
            ffi::ghostty_key_event_set_mods(self.event, key.modifiers);
            ffi::ghostty_key_event_set_consumed_mods(self.event, key.consumed_modifiers);
            ffi::ghostty_key_event_set_composing(self.event, false);
            ffi::ghostty_key_event_set_unshifted_codepoint(self.event, key.unshifted_codepoint);
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
