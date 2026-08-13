//! The survey: what Muster puts on a pane's input for the keys people press constantly.
//!
//! One matrix with one reason rather than nineteen behaviors, and its oracle is upstream,
//! so it stays a rendered snapshot instead of becoming conformance cases with nineteen
//! manufactured justifications.
//!
//! It reads the same two files the Swift suite reads, from `corpus/snapshots`. That is the
//! point of it during the port: if these had to be re-recorded to pass, they were never an
//! oracle, and the difference would say the Rust encoder is driving libghostty differently
//! from the Swift one.

mod support;

use std::fmt::Write as _;

use muster_core::input::{Key, KeyEvent, Modifiers, TerminalModeProfile};
use muster_vt::KeyEncoder;
use support::expect_snapshot;

/// The matrix, as one readable list. Every row is a keystroke a user makes constantly.
fn common_keystrokes() -> Vec<(&'static str, KeyEvent)> {
    let letter = |key, text: &str, unshifted: char, modifiers| KeyEvent {
        key,
        modifiers,
        text: text.to_string(),
        unshifted_codepoint: Some(unshifted),
        ..KeyEvent::default()
    };
    let bare = |key, modifiers| KeyEvent { key, modifiers, ..KeyEvent::default() };

    vec![
        ("a", letter(Key::KeyA, "a", 'a', Modifiers::NONE)),
        ("shift+a", letter(Key::KeyA, "A", 'a', Modifiers::SHIFT)),
        ("ctrl+c", letter(Key::KeyC, "", 'c', Modifiers::CONTROL)),
        ("enter", bare(Key::Enter, Modifiers::NONE)),
        ("shift+enter", bare(Key::Enter, Modifiers::SHIFT)),
        ("tab", bare(Key::Tab, Modifiers::NONE)),
        ("shift+tab", bare(Key::Tab, Modifiers::SHIFT)),
        ("escape", bare(Key::Escape, Modifiers::NONE)),
        ("backspace", bare(Key::Backspace, Modifiers::NONE)),
        ("arrow up", bare(Key::ArrowUp, Modifiers::NONE)),
        ("arrow down", bare(Key::ArrowDown, Modifiers::NONE)),
        ("home", bare(Key::Home, Modifiers::NONE)),
        ("end", bare(Key::End, Modifiers::NONE)),
        ("page up", bare(Key::PageUp, Modifiers::NONE)),
        ("delete", bare(Key::Delete, Modifiers::NONE)),
        ("f1", bare(Key::F1, Modifiers::NONE)),
        ("f12", bare(Key::F12, Modifiers::NONE)),
        ("alt+b", letter(Key::KeyB, "b", 'b', Modifiers::ALT)),
        ("ctrl+alt+delete", bare(Key::Delete, Modifiers::CONTROL | Modifiers::ALT)),
    ]
}

#[test]
fn what_an_unknown_pane_gets_for_the_keys_people_press_constantly() {
    expect_snapshot(&render(TerminalModeProfile::UNKNOWN_PANE), "keys-unknown-pane.txt");
}

#[test]
fn what_the_same_keys_become_once_a_panes_kitty_flags_are_known() {
    // Not reachable today - it needs mode state herdr does not expose - and recorded anyway,
    // because the difference between these two files is exactly what the upstream ask is
    // worth.
    expect_snapshot(&render(TerminalModeProfile::HERDR_TUI), "keys-herdr-tui.txt");
}

fn render(profile: TerminalModeProfile) -> String {
    let encoder = KeyEncoder::new(profile).expect("libghostty-vt should give us an encoder");
    let keystrokes = common_keystrokes();
    let width = keystrokes.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (name, event) in &keystrokes {
        let bytes = encoder.encode(event).expect("every keystroke here encodes");
        let _ = writeln!(out, "{name:width$}  {}", readable(&bytes));
    }
    out
}

/// Bytes as a reviewer reads them: ESC for 0x1b, ^C for control characters, printable
/// characters as themselves. A hex dump would be exact and unreadable, and nobody would
/// notice the day it changed.
fn readable(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(nothing)".to_string();
    }
    bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "ESC".to_string(),
            0x7f => "DEL".to_string(),
            0x00..=0x1f => format!("^{}", (byte + 0x40) as char),
            0x20 => "SP".to_string(),
            other => (*other as char).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
