//! What Muster sends a pane's program, in Muster's own words.
//!
//! The core does not encode a keystroke: the daemon does, against the pane's own terminal
//! modes, and is the only writer to the pane (MIP-3, section 6). So what leaves here is the
//! keystroke itself, as libghostty carried it, and an adapter spells it in the daemon's
//! protocol.

use super::{KeyEvent, OptionAsAlt};
use crate::mirror::backend::PaneId;

/// One thing for one pane's program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// A keystroke the keymap did not take, with option-as-alt already applied to its
    /// modifiers and text, and the setting beside it so the daemon's encoder applies the same
    /// one.
    Key { key: KeyEvent, option_as_alt: OptionAsAlt },
    /// A paste. The daemon fences it when the program asked for bracketed paste, and holds one
    /// that would run several lines as typed until somebody confirms it; `confirmed` is that
    /// confirmation.
    Paste { text: String, confirmed: bool },
    /// Text an agent or a script sends, as a paste that is never held, then Return if asked.
    Send { text: String, enter: bool },
    /// Bytes written as they are, with no encoding at all: what a `text:` binding in the
    /// config writes, and what an input method commits.
    Bytes(Vec<u8>),
}

/// Where one daemon's panes take their input from this window.
///
/// One per daemon rather than one per pane: the daemon routes each event to its pane, and a
/// connection per pane would be fifteen for a full window.
///
/// **Never blocks.** Sending happens on the window's own thread, so an event that cannot be
/// queued is dropped rather than waited for, and the implementation says so in the log.
pub trait InputSink: Send + Sync + std::fmt::Debug {
    /// Queues `event` for the pane's daemon, or says why it was not. Queued is not arrived: a
    /// connection that ends after this returns loses what was still queued on it.
    fn send(&self, pane: &PaneId, event: InputEvent) -> Result<(), NotSent>;

    /// What this sink is talking to, for the log.
    fn description(&self) -> &str;
}

/// Why an input event never left for the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSent {
    /// The window has no working connection to the pane's daemon right now.
    NotConnected,
    /// The daemon stopped reading, and what is already waiting for it is at its bound.
    Full,
    /// Larger than the daemon reads in one message, which would close the connection every
    /// pane on that daemon takes its input from.
    TooLarge { bytes: usize, limit: usize },
}

impl std::fmt::Display for NotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotSent::NotConnected => {
                f.write_str("the window is not connected to the pane's daemon right now")
            }
            NotSent::Full => {
                f.write_str("the daemon has stopped reading input, and its queue is full")
            }
            NotSent::TooLarge { bytes, limit } => write!(
                f,
                "it is {bytes} bytes, and the daemon reads at most {limit} bytes in one message"
            ),
        }
    }
}
