//! What Muster wants to happen in a pane, in Muster's own words.
//!
//! The core does not know herdr exists. It produces intents; an adapter turns them into
//! whatever the backend of the day speaks (architecture.md, the backend seam). Keeping the
//! vocabulary here rather than reusing herdr's wire types is what makes the adapter a
//! translation rather than a passthrough, and what lets a test assert on intent without a
//! daemon.

use super::KeyEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

impl ScrollDirection {
    pub fn parse(name: &str) -> Option<ScrollDirection> {
        match name {
            "up" => Some(ScrollDirection::Up),
            "down" => Some(ScrollDirection::Down),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ScrollDirection::Up => "up",
            ScrollDirection::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneIntent {
    /// Bytes for the pane's PTY, already encoded.
    Input(Vec<u8>),

    /// Text for the pane, left for the backend to encode.
    ///
    /// Distinct from `Input` because the backend can do this better than we can: it knows
    /// the pane's real terminal modes, so it fences a paste correctly where Muster would
    /// have to guess (`docs/observations/herdr-0.8.0.md` section 5).
    Text(String),

    /// A named key, left for the backend to encode against the pane's real modes.
    ///
    /// The escape hatch for the keys where guessing is known to be wrong - the arrows above
    /// all, since a program that called `smkx` wants `SS3` and one that did not wants `CSI`,
    /// and nothing on the control stream says which.
    Key { name: String },

    /// A wheel movement, which the backend routes against the pane's mouse mode.
    Scroll { direction: ScrollDirection, lines: u16 },

    /// How big the pane's grid should be, in cells.
    ///
    /// Not on the input path at all, and here anyway because this is the one channel that can
    /// carry it: a pane's size follows whichever client is driving it, and driving it is what
    /// this channel does. The window's own resizes never come through here - the surface's PTY
    /// carries those, which is why the bridge needs no channel for them - so the one caller is
    /// Muster letting go, handing a pane back at a size the daemon will lay it out at rather
    /// than at the size of a window that no longer exists.
    Resize { columns: u16, rows: u16 },
}

/// Where a pane's intents go.
///
/// Two implementations exist and they differ in a way the core must not care about: one
/// writes bytes onto a control stream the pane already holds open, the other asks the
/// daemon to encode and costs a round trip. `deliver` returning whether it arrived is what
/// lets the caller degrade instead of silently swallowing input.
pub trait PaneChannel: Send + Sync {
    /// Sends one intent, and says whether it got there.
    fn deliver(&self, intent: &PaneIntent) -> bool;

    /// Whether this channel can encode an intent the client cannot - text and named keys.
    ///
    /// A control stream alone cannot: it is a raw pipe to the PTY, so anything sent on it
    /// has already been encoded by us, against a guess.
    fn encodes_server_side(&self) -> bool;

    /// Named for logs, so a failure says which channel dropped the input.
    fn description(&self) -> &str;
}

/// Why a keystroke could not be turned into bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError(pub String);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}

/// Turns a keystroke into the bytes a terminal program expects.
///
/// A trait so the core can own the input pipeline without owning an encoder: the real one
/// is libghostty-vt's, and a test wants a table it can read. Encoding is the one part of
/// input that must agree exactly with a published implementation, so the seam is here
/// rather than a reimplementation.
pub trait KeyEncoding: Send + Sync {
    /// The bytes for this keystroke, or empty when the keystroke produces none - a bare
    /// modifier, or any key while an input method is composing.
    fn encode(&self, key: &KeyEvent) -> Result<Vec<u8>, EncodeError>;
}
