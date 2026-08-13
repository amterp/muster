//! What Muster does with a keystroke before the pane sees it.
//!
//! Input precedence is fixed (architecture.md): the keymap gets first refusal on every
//! chord, and only what it declines is reported toward the focused pane. That ordering is
//! structural here from the first keystroke rather than retrofitted later, because
//! retrofitting it means finding every place that already sends bytes.
//!
//! The bindings here are defaults, not configuration. A config file replaces the table
//! without changing the path it is consulted on.

use std::collections::HashMap;

use super::{Key, KeyAction, KeyEvent, Modifiers};

/// A chord: which key, under which modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Binding {
    pub fn new(key: Key, modifiers: Modifiers) -> Binding {
        Binding { key, modifiers }
    }
}

/// The operations a chord can be bound to.
///
/// Empty until there is something to bind. Kept as a type rather than a comment so the
/// dispatcher has somewhere to grow, and so `Resolution` is not a lie about a shape that
/// does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {}

/// What a chord resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Muster handles it; the pane never sees it.
    Action(Action),
    /// Muster substitutes these bytes for whatever the encoder would have produced.
    Text(Vec<u8>),
    /// The backend encodes this one, under this name.
    ///
    /// For the keys where encoding locally is known to be wrong. Muster guesses the pane's
    /// terminal modes and the daemon does not have to.
    ServerEncoded(String),
    /// Not bound. Report it to the pane.
    Unbound,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: HashMap<Binding, Resolution>,
}

impl Keymap {
    pub fn new(bindings: HashMap<Binding, Resolution>) -> Keymap {
        Keymap { bindings }
    }

    pub fn resolve(&self, key: &KeyEvent) -> Resolution {
        if key.action == KeyAction::Release {
            return Resolution::Unbound;
        }
        let held = key.modifiers.intersection(Modifiers::CHORD);
        self.bindings.get(&Binding::new(key.key, held)).cloned().unwrap_or(Resolution::Unbound)
    }
}

impl Default for Keymap {
    /// What Muster binds out of the box.
    ///
    /// The mode-sensitive keys go in first and the local bindings overwrite them, so a
    /// collision goes to the local one: a chord someone chose to bind should not lose to a
    /// key that merely wants encoding help.
    fn default() -> Keymap {
        let mut bindings = HashMap::new();
        bindings.extend(mode_sensitive_keys());
        bindings.extend(macos_text_editing());
        Keymap::new(bindings)
    }
}

/// The line-editing chords macOS users expect a terminal to honor.
///
/// These are not terminal conventions - no program asks for them and no mode enables them.
/// They are the text-editing shortcuts every other macOS app has, which people reasonably
/// keep pressing in a terminal, and each one maps onto the readline control code that does
/// the same job. Without them ⌘⌫ deletes a single character and looks broken.
///
/// Taken from ghostty, whose macOS build binds exactly these five and calls them "natural
/// text editing" (`src/config/Config.zig`, in the macOS keybind defaults). Matching it is
/// the point rather than a coincidence: Muster promises the platform's own keybindings, and
/// a person moving between the two terminals should not have to learn which is which.
///
/// They also sidestep the mode problem, since a control code means the same thing whatever
/// the pane has negotiated.
fn macos_text_editing() -> Vec<(Binding, Resolution)> {
    vec![
        // Start and end of line, which readline spells ctrl+A and ctrl+E.
        (Binding::new(Key::ArrowLeft, Modifiers::SUPER), Resolution::Text(vec![0x01])),
        (Binding::new(Key::ArrowRight, Modifiers::SUPER), Resolution::Text(vec![0x05])),
        // Delete to start of line: readline's unix-line-discard.
        (Binding::new(Key::Backspace, Modifiers::SUPER), Resolution::Text(vec![0x15])),
        // Word motion, as an escape prefix rather than a control code.
        (Binding::new(Key::ArrowLeft, Modifiers::ALT), Resolution::Text(vec![0x1b, b'b'])),
        (Binding::new(Key::ArrowRight, Modifiers::ALT), Resolution::Text(vec![0x1b, b'f'])),
    ]
}

/// The keys whose correct encoding depends on a mode Muster cannot see.
///
/// The arrows, and only the arrows, for a measured reason. Application cursor mode decides
/// between `ESC O A` and `ESC [ A`, and a program that trusts terminfo accepts only the
/// first: `less` calls `smkx` on startup and then rings the bell at anything else. `vim`
/// accepts both, which is why one program is not a survey. Nothing else in the guess was
/// measured to break - shift+enter, dead keys and control chords all survive - so nothing
/// else is routed the slow way.
///
/// Unmodified only. herdr's key vocabulary does accept chords like `shift+up`, but a
/// modified arrow is not what a pager reads, and every routed key costs a round trip.
fn mode_sensitive_keys() -> Vec<(Binding, Resolution)> {
    vec![
        (Binding::new(Key::ArrowUp, Modifiers::NONE), Resolution::ServerEncoded("up".into())),
        (Binding::new(Key::ArrowDown, Modifiers::NONE), Resolution::ServerEncoded("down".into())),
        (Binding::new(Key::ArrowLeft, Modifiers::NONE), Resolution::ServerEncoded("left".into())),
        (Binding::new(Key::ArrowRight, Modifiers::NONE), Resolution::ServerEncoded("right".into())),
    ]
}
