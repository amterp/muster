//! Terminal modes, by the numbers a program sets them with.

use std::fmt;

include!(concat!(env!("OUT_DIR"), "/modes.rs"));

/// One mode, as `CSI ? n h` (DEC private) or `CSI n h` (ANSI) names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode {
    pub value: u16,
    pub ansi: bool,
}

impl Mode {
    pub const INSERT: Mode = Mode::ansi(4);
    pub const LINEFEED: Mode = Mode::ansi(20);
    pub const CURSOR_KEYS: Mode = Mode::dec(1);
    pub const COLUMN_132: Mode = Mode::dec(3);
    pub const ORIGIN: Mode = Mode::dec(6);
    pub const WRAPAROUND: Mode = Mode::dec(7);
    pub const CURSOR_VISIBLE: Mode = Mode::dec(25);
    pub const ALT_SCREEN_LEGACY: Mode = Mode::dec(47);
    pub const KEYPAD_KEYS: Mode = Mode::dec(66);
    pub const FOCUS_EVENT: Mode = Mode::dec(1004);
    pub const ALT_SCROLL: Mode = Mode::dec(1007);
    pub const ALT_SCREEN: Mode = Mode::dec(1047);
    pub const SAVE_CURSOR: Mode = Mode::dec(1048);
    pub const ALT_SCREEN_SAVE: Mode = Mode::dec(1049);
    pub const BRACKETED_PASTE: Mode = Mode::dec(2004);
    pub const SYNC_OUTPUT: Mode = Mode::dec(2026);
    pub const GRAPHEME_CLUSTER: Mode = Mode::dec(2027);
    pub const COLOR_SCHEME_REPORT: Mode = Mode::dec(2031);

    pub const fn dec(value: u16) -> Mode {
        Mode { value, ansi: false }
    }

    pub const fn ansi(value: u16) -> Mode {
        Mode { value, ansi: true }
    }

    /// Every mode the pinned libghostty-vt knows, in `modes.h`'s order.
    pub fn all() -> impl Iterator<Item = Mode> {
        MODES.iter().map(|&(_, value, ansi)| Mode { value, ansi })
    }

    /// The header's name for it, for messages a person reads.
    pub fn name(self) -> &'static str {
        MODES
            .iter()
            .find(|&&(_, value, ansi)| value == self.value && ansi == self.ansi)
            .map_or("UNKNOWN", |&(name, _, _)| name)
    }

    /// The sequence that puts this mode in `enabled`.
    pub fn sequence(self, enabled: bool) -> String {
        let prefix = if self.ansi { "" } else { "?" };
        let suffix = if enabled { 'h' } else { 'l' };
        format!("\x1b[{prefix}{}{suffix}", self.value)
    }

    pub(crate) fn packed(self) -> u16 {
        (self.value & 0x7FFF) | (u16::from(self.ansi) << 15)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.ansi { "" } else { "?" };
        write!(f, "{prefix}{} ({})", self.value, self.name())
    }
}
