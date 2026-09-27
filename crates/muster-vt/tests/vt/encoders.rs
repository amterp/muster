//! Input encoded against the pane's real modes, read from its terminal.
//!
//! The encoders are libghostty-vt's; what these pin is that the modes a program set reach
//! them, which is the whole reason the daemon encodes input rather than the app guessing
//! at a profile (MIP-3 section 6).

use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers, OptionAsAlt};
use muster_vt::{
    EncoderError, KeyEncoder, KeyModes, MouseAction, MouseButton, MouseEncoder, MouseEvent,
    MouseGeometry, RawKeyEvent, Terminal, encode_paste, paste_is_safe,
};

fn terminal_after(bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(80, 24).expect("libghostty-vt gives us a terminal");
    terminal.write(bytes);
    terminal
}

fn key_from(terminal: &Terminal, key: Key) -> Vec<u8> {
    let mut encoder =
        KeyEncoder::new(KeyModes::default()).expect("libghostty-vt gives us an encoder");
    encoder.configure_from(terminal, OptionAsAlt::Never);
    encoder.encode(&KeyEvent::press(key)).expect("the key encodes")
}

#[test]
fn an_arrow_follows_the_programs_cursor_key_mode() {
    assert_eq!(key_from(&terminal_after(b""), Key::ArrowUp), b"\x1b[A");
    assert_eq!(key_from(&terminal_after(b"\x1b[?1h"), Key::ArrowUp), b"\x1bOA");
}

#[test]
fn escape_follows_the_programs_kitty_flags() {
    assert_eq!(key_from(&terminal_after(b""), Key::Escape), b"\x1b");
    assert_eq!(key_from(&terminal_after(b"\x1b[>1u"), Key::Escape), b"\x1b[27u");
}

/// libghostty's code for Escape, as a surface hands it over.
const ESCAPE: u32 = 120;

fn raw_escape(code: u32) -> RawKeyEvent<'static> {
    RawKeyEvent {
        action: KeyAction::Press,
        code,
        modifiers: 0,
        consumed_modifiers: 0,
        text: "",
        unshifted_codepoint: 0,
        composing: false,
    }
}

#[test]
fn a_key_numbered_as_libghostty_numbers_it_encodes_as_the_same_key() {
    let terminal = terminal_after(b"\x1b[>1u");
    let mut encoder =
        KeyEncoder::new(KeyModes::default()).expect("libghostty-vt gives us an encoder");
    encoder.configure_from(&terminal, OptionAsAlt::Never);
    assert_eq!(encoder.encode_raw(&raw_escape(ESCAPE)), Ok(key_from(&terminal, Key::Escape)));
    assert_eq!(
        encoder.encode_raw(&raw_escape(100_000)),
        Err(EncoderError::UnknownKey(100_000)),
        "a code past the last key is refused rather than handed to libghostty"
    );
}

fn mouse_from(terminal: &Terminal, event: MouseEvent) -> Vec<u8> {
    let mut encoder = MouseEncoder::new().expect("libghostty-vt gives us an encoder");
    encoder.configure_from(terminal);
    encoder.set_geometry(MouseGeometry {
        screen_pixels: (800, 480),
        cell_pixels: (10, 20),
        padding: (0, 0, 0, 0),
    });
    encoder.encode(&event).expect("the event encodes")
}

fn click(button: MouseButton) -> MouseEvent {
    MouseEvent {
        action: MouseAction::Press,
        button: Some(button),
        modifiers: Modifiers::NONE,
        // Column 3, row 2, one-based: the middle of the cell at (2, 1).
        position: (25.0, 30.0),
    }
}

#[test]
fn a_click_reports_nothing_until_the_program_asks() {
    assert_eq!(mouse_from(&terminal_after(b""), click(MouseButton::Left)), b"");
}

#[test]
fn a_click_reports_in_the_format_the_program_chose() {
    let sgr = terminal_after(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(mouse_from(&sgr, click(MouseButton::Left)), b"\x1b[<0;3;2M");
    assert_eq!(mouse_from(&sgr, click(MouseButton::WheelDown)), b"\x1b[<65;3;2M");
}

#[test]
fn a_paste_is_fenced_only_when_the_program_asked() {
    assert_eq!(encode_paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
    assert_eq!(encode_paste("a\nb", false), b"a\rb");
}

#[test]
fn a_paste_cannot_close_the_fence_early() {
    let hostile = "x\x1b[201~rm -rf ~\n";
    assert!(!paste_is_safe(hostile));
    let encoded = encode_paste(hostile, true);
    let inside = &encoded[6..encoded.len() - 6];
    assert!(!inside.windows(6).any(|w| w == b"\x1b[201~"), "fence closed early: {encoded:?}");
    assert!(paste_is_safe("one line"));
}
