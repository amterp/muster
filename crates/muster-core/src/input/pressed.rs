//! A chord pressed on somebody's behalf, as the keystroke a keyboard would have sent.

use super::{Chord, Key, KeyEvent, Modifiers};

/// Refuses the first of a `pane send`'s keys that names no key, before anything is sent.
///
/// Checked before the send rather than left to the daemon, which can only drop a send it cannot
/// read: an input connection answers nothing, so the caller would hear success.
pub fn check_sent_keys(keys: &[String]) -> Result<(), String> {
    for key in keys {
        Chord::parse(key).map_err(|refusal| format!("nothing was sent: {refusal}"))?;
    }
    Ok(())
}

impl Chord {
    /// The keystroke for this chord, filled in as a US layout would fill it.
    ///
    /// For `muster pane send --key`, where nobody's keyboard is involved, so nothing hands over
    /// the text a key produces - and the encoder needs it: with no text, a digit or a letter
    /// encodes to nothing in legacy mode. US because it is the layout whose characters the chord
    /// names are spelled in, so `--key 2` types a 2 whatever the machine's own layout is.
    ///
    /// Control and command produce no text, as on a Mac; the encoder builds `ctrl+c` from the
    /// unshifted codepoint. Alt keeps its text and is sent as alt, since somebody naming alt in a
    /// chord means the modifier rather than option's compose layer.
    pub fn pressed(self) -> KeyEvent {
        let characters = us_characters(self.key);
        let shifted = self.modifiers.contains(Modifiers::SHIFT);
        let text = match characters {
            _ if self.modifiers.contains(Modifiers::CONTROL)
                || self.modifiers.contains(Modifiers::SUPER) =>
            {
                String::new()
            }
            Some((_, upper)) if shifted => upper.to_string(),
            Some((lower, _)) => lower.to_string(),
            None => String::new(),
        };
        let consumed = if shifted && !text.is_empty() { Modifiers::SHIFT } else { Modifiers::NONE };
        KeyEvent {
            modifiers: self.modifiers,
            consumed_modifiers: consumed,
            text,
            unshifted_codepoint: characters.map(|(lower, _)| lower),
            ..KeyEvent::press(self.key)
        }
    }
}

/// The characters a key types on a US layout, without and with shift.
fn us_characters(key: Key) -> Option<(char, char)> {
    let letter = |lower: char| Some((lower, lower.to_ascii_uppercase()));
    match key {
        Key::KeyA => letter('a'),
        Key::KeyB => letter('b'),
        Key::KeyC => letter('c'),
        Key::KeyD => letter('d'),
        Key::KeyE => letter('e'),
        Key::KeyF => letter('f'),
        Key::KeyG => letter('g'),
        Key::KeyH => letter('h'),
        Key::KeyI => letter('i'),
        Key::KeyJ => letter('j'),
        Key::KeyK => letter('k'),
        Key::KeyL => letter('l'),
        Key::KeyM => letter('m'),
        Key::KeyN => letter('n'),
        Key::KeyO => letter('o'),
        Key::KeyP => letter('p'),
        Key::KeyQ => letter('q'),
        Key::KeyR => letter('r'),
        Key::KeyS => letter('s'),
        Key::KeyT => letter('t'),
        Key::KeyU => letter('u'),
        Key::KeyV => letter('v'),
        Key::KeyW => letter('w'),
        Key::KeyX => letter('x'),
        Key::KeyY => letter('y'),
        Key::KeyZ => letter('z'),
        Key::Digit1 => Some(('1', '!')),
        Key::Digit2 => Some(('2', '@')),
        Key::Digit3 => Some(('3', '#')),
        Key::Digit4 => Some(('4', '$')),
        Key::Digit5 => Some(('5', '%')),
        Key::Digit6 => Some(('6', '^')),
        Key::Digit7 => Some(('7', '&')),
        Key::Digit8 => Some(('8', '*')),
        Key::Digit9 => Some(('9', '(')),
        Key::Digit0 => Some(('0', ')')),
        Key::Space => Some((' ', ' ')),
        Key::Minus => Some(('-', '_')),
        Key::Equal => Some(('=', '+')),
        Key::BracketLeft => Some(('[', '{')),
        Key::BracketRight => Some((']', '}')),
        Key::Backslash => Some(('\\', '|')),
        Key::Semicolon => Some((';', ':')),
        Key::Quote => Some(('\'', '"')),
        Key::Comma => Some((',', '<')),
        Key::Period => Some(('.', '>')),
        Key::Slash => Some(('/', '?')),
        Key::Backquote => Some(('`', '~')),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(chord: &str) -> KeyEvent {
        Chord::parse(chord).expect("a chord").pressed()
    }

    #[test]
    fn a_digit_or_letter_types_its_character_and_shift_types_the_other_one() {
        assert_eq!(pressed("2").text, "2");
        assert_eq!(pressed("p").text, "p");
        assert_eq!(pressed("shift+p").text, "P");
        assert_eq!(pressed("shift+p").consumed_modifiers, Modifiers::SHIFT);
        assert_eq!(pressed("shift+/").text, "?");
        assert_eq!(pressed("p").unshifted_codepoint, Some('p'));
    }

    #[test]
    fn control_types_nothing_and_leaves_the_encoder_the_key() {
        let control_c = pressed("ctrl+c");
        assert_eq!(control_c.text, "");
        assert_eq!(control_c.unshifted_codepoint, Some('c'));
        assert!(control_c.modifiers.contains(Modifiers::CONTROL));
        assert_eq!(pressed("alt+b").text, "b", "alt is sent as alt, which needs the text");
    }

    #[test]
    fn a_key_with_no_character_has_no_text() {
        let down = pressed("down");
        assert_eq!(down.key, Key::ArrowDown);
        assert_eq!(down.text, "");
        assert_eq!(down.unshifted_codepoint, None);
        assert_eq!(pressed("esc").key, Key::Escape);
    }
}
