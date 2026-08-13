//! A keystroke, in Muster's own terms.

use std::ops::BitOr;

use super::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyAction {
    #[default]
    Press,
    Release,
    /// Auto-repeat. Distinct from a press because the kitty protocol reports it
    /// separately, and a program that tracks held keys needs the difference.
    Repeated,
}

impl KeyAction {
    pub fn parse(name: &str) -> Option<KeyAction> {
        match name {
            "press" => Some(KeyAction::Press),
            "release" => Some(KeyAction::Release),
            "repeated" => Some(KeyAction::Repeated),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            KeyAction::Press => "press",
            KeyAction::Release => "release",
            KeyAction::Repeated => "repeated",
        }
    }
}

/// A keystroke, OS-free on purpose.
///
/// The shell fills this in from whatever its platform hands it, and the keymap, the encoder
/// and the tests downstream read only this. That is what lets key handling be decided in a
/// headless core and exercised as a table (`docs/testing.md`), rather than needing a window
/// and a keyboard.
///
/// The fields are the ones a terminal encoder needs, and no others. `text` and
/// `unshifted_codepoint` both exist because they answer different questions: what the
/// layout produced, and what the key is called when modifiers are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub action: KeyAction,
    pub key: Key,
    pub modifiers: Modifiers,

    /// The modifiers the keyboard layout already spent producing `text`.
    ///
    /// Encoders must not report these a second time: on a German layout, option+8 produces
    /// `{`, and an encoder told both "option is down" and "the text is {" would send an
    /// escape sequence for alt+{ instead of the brace the user typed.
    pub consumed_modifiers: Modifiers,

    /// What the layout produced, if anything. Empty for keys with no text, such as arrows.
    pub text: String,

    /// The codepoint this key produces with no modifiers at all.
    ///
    /// The kitty protocol reports it so an application can recognize a chord by the key the
    /// user sees printed on the cap, independent of shift state and layout.
    pub unshifted_codepoint: Option<char>,

    /// Whether an input method is mid-composition.
    ///
    /// A composing keystroke belongs to the input method, not to the pane: it must not be
    /// encoded and sent, or typing Japanese would deliver the romaji as well as the result.
    pub is_composing: bool,
}

impl KeyEvent {
    /// A plain press, which every other shape is a variation on.
    pub fn press(key: Key) -> KeyEvent {
        KeyEvent { key, ..KeyEvent::default() }
    }
}

/// Written out rather than derived, so that the generated key vocabulary stays a list of
/// keys and does not have to nominate one of them as special.
impl Default for KeyEvent {
    fn default() -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key: Key::Unidentified,
            modifiers: Modifiers::NONE,
            consumed_modifiers: Modifiers::NONE,
            text: String::new(),
            unshifted_codepoint: None,
            is_composing: false,
        }
    }
}

/// Which modifier keys were down.
///
/// The bit positions match libghostty-vt's `GhosttyMods` so the encoder seam is a cast
/// rather than a translation. That is a deliberate coupling to a published ABI, and the
/// key-encoder corpus pins it: if a pin bump renumbers these, a case fails rather than
/// every chord quietly encoding as something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Modifiers(pub u16);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1 << 0);
    pub const CONTROL: Modifiers = Modifiers(1 << 1);
    /// Alt on a PC keyboard, option on a Mac.
    pub const ALT: Modifiers = Modifiers(1 << 2);
    /// Command on a Mac, Windows key elsewhere.
    pub const SUPER: Modifiers = Modifiers(1 << 3);
    pub const CAPS_LOCK: Modifiers = Modifiers(1 << 4);
    pub const NUM_LOCK: Modifiers = Modifiers(1 << 5);

    /// Side bits. Only meaningful when the matching modifier bit is set, and only on
    /// platforms that report the difference.
    pub const SHIFT_IS_RIGHT: Modifiers = Modifiers(1 << 6);
    pub const CONTROL_IS_RIGHT: Modifiers = Modifiers(1 << 7);
    pub const ALT_IS_RIGHT: Modifiers = Modifiers(1 << 8);
    pub const SUPER_IS_RIGHT: Modifiers = Modifiers(1 << 9);

    /// The modifiers that pick a binding. A chord is the same chord whichever side of the
    /// keyboard supplied its command key.
    pub const CHORD: Modifiers =
        Modifiers(Self::SHIFT.0 | Self::CONTROL.0 | Self::ALT.0 | Self::SUPER.0);

    /// The wire names for each bit, in bit order.
    ///
    /// Muster's vocabulary has to survive leaving this process - into the conformance
    /// corpus, into the log, and into the schema the shell and core speak (MIP-1). A
    /// bitmask does none of that legibly: `520` in a case file tells a reader nothing, and
    /// silently means something different if the bits are ever renumbered.
    pub const ALL_NAMES: [(&'static str, Modifiers); 10] = [
        ("shift", Modifiers::SHIFT),
        ("control", Modifiers::CONTROL),
        ("alt", Modifiers::ALT),
        ("super", Modifiers::SUPER),
        ("capsLock", Modifiers::CAPS_LOCK),
        ("numLock", Modifiers::NUM_LOCK),
        ("shiftIsRight", Modifiers::SHIFT_IS_RIGHT),
        ("controlIsRight", Modifiers::CONTROL_IS_RIGHT),
        ("altIsRight", Modifiers::ALT_IS_RIGHT),
        ("superIsRight", Modifiers::SUPER_IS_RIGHT),
    ];

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub fn intersection(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 & other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The names of the bits that are set, in bit order so two runs agree.
    pub fn names(self) -> Vec<&'static str> {
        Modifiers::ALL_NAMES
            .iter()
            .filter(|(_, bit)| self.contains(*bit))
            .map(|(name, _)| *name)
            .collect()
    }

    /// Reads a set of names, or nothing if any is not a modifier.
    ///
    /// Fails rather than ignoring the unknown one: a case file that says `"comand"` should
    /// say so, not quietly test an unmodified key and pass.
    pub fn parse(names: &[String]) -> Option<Modifiers> {
        let mut result = Modifiers::NONE;
        for name in names {
            let (_, bit) = Modifiers::ALL_NAMES.iter().find(|(known, _)| known == name)?;
            result = result | *bit;
        }
        Some(result)
    }
}

impl BitOr for Modifiers {
    type Output = Modifiers;

    fn bitor(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
}
