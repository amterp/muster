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

use super::{Binding, KeyEvent, Keymap, Modifiers, OptionAsAlt, TerminalModeProfile};

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

    /// The keystroke as the encoder should see it, with option read the way the file asks.
    ///
    /// Setting the encoder's own option flag is necessary and not sufficient, which is the
    /// part that is easy to get wrong and impossible to see. The encoder computes its
    /// effective modifiers as everything held minus everything the layout spent, and macOS
    /// always reports option as spent when option composed a character. So a held-and-spent
    /// option arrives as no option at all, the alt-prefix branch is never reached, and the
    /// flag that would have enabled it is read by nobody. Every conformance case that left
    /// consumed modifiers out passed while every real keystroke did the opposite.
    ///
    /// So the spend is taken back here, and the text is replaced with what the layout would
    /// have produced without option - the second reading the shell reports for exactly this.
    /// Ghostty does the same thing one layer up, by re-translating the event before it ever
    /// reaches the encoder (`macos/Sources/Ghostty/Surface View/SurfaceView_AppKit.swift`,
    /// `keyDown`); the difference is where the decision sits, not what it decides.
    ///
    /// Untouched when option is not held, or when the config leaves option composing, so the
    /// default path costs a modifier test and nothing else.
    pub fn as_alt(&self, key: &KeyEvent) -> Option<KeyEvent> {
        if !key.modifiers.contains(Modifiers::ALT) || !self.option_is_alt(key.modifiers) {
            return None;
        }
        Some(KeyEvent {
            text: key.text_without_option.clone(),
            consumed_modifiers: key.consumed_modifiers.without(Modifiers::ALT),
            ..key.clone()
        })
    }

    /// Whether the option key being held is one the config says means alt.
    ///
    /// The side comes from the event, because a chord with the left option down and the
    /// right one configured is an ordinary composition rather than a meta chord. A keystroke
    /// that names no side is a left one: that is what the encoding treats as the default and
    /// what a shell that cannot tell the sides apart would report.
    fn option_is_alt(&self, held: Modifiers) -> bool {
        match self.option_as_alt {
            OptionAsAlt::Never => false,
            OptionAsAlt::Always => true,
            OptionAsAlt::LeftOnly => !held.contains(Modifiers::ALT_IS_RIGHT),
            OptionAsAlt::RightOnly => held.contains(Modifiers::ALT_IS_RIGHT),
        }
    }
}
