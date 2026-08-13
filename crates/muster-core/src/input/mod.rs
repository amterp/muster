//! What happens when you type: the vocabulary, the precedence, and the route out.

mod composition;
mod key;
mod key_event;
mod keymap;
mod mode_profile;
mod pane_channel;
mod pane_input;

pub use composition::{Outcome as CompositionOutcome, outcome as composition_outcome};
pub use key::Key;
pub use key_event::{KeyAction, KeyEvent, Modifiers};
pub use keymap::{Binding, Keymap, Resolution};
pub use mode_profile::{OptionAsAlt, TerminalModeProfile, kitty_flags};
pub use pane_channel::{EncodeError, KeyEncoding, PaneChannel, PaneIntent, ScrollDirection};
pub use pane_input::PaneInput;
