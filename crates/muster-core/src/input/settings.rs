//! What a config file says about keystrokes on their way to a pane.
//!
//! Two knobs that answer to the same question - what should this keypress become - and reach
//! the input path at the same moment, so they travel together rather than as two unrelated
//! fields threaded through the same calls.
//!
//! They land in different places once they get there. `option_as_alt` is a property of the
//! terminal the keystroke is being encoded for, so it goes into [`TerminalModeProfile`];
//! `text` is a substitution made before any encoder sees the key, so it goes into
//! [`Keymap`]. Assembling both here keeps the config file's shape from deciding the input
//! path's shape.

use std::collections::BTreeMap;

use super::{Binding, Keymap, OptionAsAlt, TerminalModeProfile};

/// The config file's answers about typing, with Muster's own where it said nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneInputSettings {
    /// Whether the macOS option key means alt or composes a character.
    pub option_as_alt: OptionAsAlt,

    /// Chords that stand for literal bytes rather than for whatever the encoder would make
    /// of them. Ordered so that two runs of the same config produce the same keymap.
    pub text: BTreeMap<Binding, Vec<u8>>,
}

impl PaneInputSettings {
    /// The modes to encode against, with the one field here that is a preference rather than
    /// a guess filled in.
    ///
    /// Everything else stays [`TerminalModeProfile::UNKNOWN_PANE`], which is the documented
    /// assumption about a pane whose real modes herdr will not show us. Nothing in a config
    /// file should be able to claim a pane negotiated the kitty protocol: that is a fact
    /// about a running program, and guessing it high is the failure that mode profile exists
    /// to avoid.
    pub fn profile(&self) -> TerminalModeProfile {
        TerminalModeProfile {
            option_acts_as_alt: self.option_as_alt,
            ..TerminalModeProfile::UNKNOWN_PANE
        }
    }

    /// What Muster binds, with this file's text bindings over it.
    pub fn keymap(&self) -> Keymap {
        Keymap::with_text(self.text.clone())
    }
}
