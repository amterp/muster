//! What happens when you type: the vocabulary, the precedence, and the route out.

mod bindings;
mod composition;
mod key;
mod key_event;
mod keymap;
mod option_as_alt;
mod pane_channel;
mod pane_input;
mod settings;

pub use bindings::{Action, Bindings, Chord, NumberedChords, ghostty_equivalent};
pub use composition::{Outcome as CompositionOutcome, outcome as composition_outcome};
pub use key::Key;
pub use key_event::{KeyAction, KeyEvent, Modifiers};
pub use keymap::{Binding, Keymap, Resolution, TEXT_EDITING};
pub use option_as_alt::OptionAsAlt;
pub use pane_channel::{InputEvent, InputSink, Mouse, MouseAction, MouseButton, NotSent, Wheel};
pub use pane_input::PaneInput;
pub use settings::PaneInputSettings;
