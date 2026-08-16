//! What Muster does rather than what a pane does, and which chord asks for it.
//!
//! Distinct from `keymap`, which decides what happens to a keystroke on its way to a pane.
//! These are the window's own actions - splitting, focusing, resizing - and on macOS they are
//! dispatched by the menu rather than by matching chords as they arrive. That is deliberate:
//! a key equivalent on a menu item is how the platform decides what a chord means, so it
//! keeps working when somebody has rebound it in System Settings, and the item shows the
//! shortcut it is actually on (`AppMenu.swift`).
//!
//! So what lives here is the table rather than the dispatch: the default chord for every
//! action, and whatever a config file says instead. The shell reads it and builds a menu; a
//! shell on a platform with no menu bar would read the same table and match chords itself.
//!
//! The spelling is the config file's, not the wire's. Somebody writing `cmd+shift+d` should
//! not have to know that Muster calls it `super` and `KeyD` internally.

use std::collections::BTreeMap;

use super::{Key, Modifiers};

/// Something Muster does to its own window.
///
/// Named for what a person is asking for. The list is the vocabulary a config file binds and
/// a shell dispatches, so adding one is one entry here and one place in each shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    NewTab,
    NextTab,
    PreviousTab,
    /// Shows the tab at this place in the window's tab order, counting from one.
    ///
    /// One variant carrying a number rather than nine spelled out, because they differ only
    /// by the digit and a list of nine near-identical arms is a list nobody keeps in step.
    /// Only 1 to 9 are ever built - [`Action::ALL`] is the whole vocabulary - so the places
    /// beyond that have no name, no chord and no menu item.
    FocusTab(u8),
    SplitRight,
    SplitDown,
    SplitLeft,
    SplitUp,
    ClosePane,
    NextPane,
    PreviousPane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    Zoom,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ToggleSidebar,
    ShowShortcuts,
}

impl Action {
    /// Every action, which is also the order a menu lists them in.
    ///
    /// Deliberately not alphabetical: a menu is read top to bottom, and the order here is what
    /// somebody scanning it expects - making something, then arranging it, then moving around
    /// it. A shell that sorted these would produce a menu nobody can find anything in.
    pub const ALL: [Action; 33] = [
        Action::NewTab,
        Action::NextTab,
        Action::PreviousTab,
        Action::FocusTab(1),
        Action::FocusTab(2),
        Action::FocusTab(3),
        Action::FocusTab(4),
        Action::FocusTab(5),
        Action::FocusTab(6),
        Action::FocusTab(7),
        Action::FocusTab(8),
        Action::FocusTab(9),
        Action::SplitRight,
        Action::SplitDown,
        Action::SplitLeft,
        Action::SplitUp,
        Action::ClosePane,
        Action::NextPane,
        Action::PreviousPane,
        Action::FocusLeft,
        Action::FocusRight,
        Action::FocusUp,
        Action::FocusDown,
        Action::ResizeLeft,
        Action::ResizeRight,
        Action::ResizeUp,
        Action::ResizeDown,
        Action::Zoom,
        Action::IncreaseFontSize,
        Action::DecreaseFontSize,
        Action::ResetFontSize,
        Action::ToggleSidebar,
        Action::ShowShortcuts,
    ];

    /// The name a config file, a log line and the seam all spell it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::NewTab => "new_tab",
            Action::NextTab => "next_tab",
            Action::PreviousTab => "previous_tab",
            // A table rather than a format, because the answer is a `&'static str` and a name
            // built at a call site cannot be one. A place outside it comes back unnameable
            // rather than borrowing another place's name, so `parse` refuses it and nothing
            // silently binds ⌘4 to the wrong tab.
            Action::FocusTab(place) => {
                TAB_PLACES.get(usize::from(place).wrapping_sub(1)).copied().unwrap_or("focus_tab")
            }
            Action::SplitRight => "split_right",
            Action::SplitDown => "split_down",
            Action::SplitLeft => "split_left",
            Action::SplitUp => "split_up",
            Action::ClosePane => "close_pane",
            Action::NextPane => "next_pane",
            Action::PreviousPane => "previous_pane",
            Action::FocusLeft => "focus_left",
            Action::FocusRight => "focus_right",
            Action::FocusUp => "focus_up",
            Action::FocusDown => "focus_down",
            Action::ResizeLeft => "resize_left",
            Action::ResizeRight => "resize_right",
            Action::ResizeUp => "resize_up",
            Action::ResizeDown => "resize_down",
            Action::Zoom => "zoom",
            Action::IncreaseFontSize => "increase_font_size",
            Action::DecreaseFontSize => "decrease_font_size",
            Action::ResetFontSize => "reset_font_size",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::ShowShortcuts => "show_shortcuts",
        }
    }

    pub fn parse(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.as_str() == name)
    }

    /// The chord Muster ships it on, if it ships on one.
    ///
    /// Ghostty's, wherever Ghostty has one. Somebody arriving from the terminal Muster embeds
    /// should not have to learn a second set of chords for the same actions - and the ones
    /// Ghostty has no answer for sit where the pattern says they should: resize is focus with
    /// one more finger, so moving to a pane and growing it are the same hand position.
    ///
    /// `None` is parity too, and the more careful half of it. Ghostty has `new_split:left` and
    /// `new_split:up` as actions and ships neither on a chord, so Muster does the same: the
    /// action exists and `[keymap]` turns it on, and nobody's ⌘D quietly starts meaning
    /// something else. It is the same state a config file produces by unbinding.
    pub fn default_chord(self) -> Option<Chord> {
        let command = Modifiers::SUPER;
        let shifted = Modifiers(Modifiers::SUPER.0 | Modifiers::SHIFT.0);
        let optioned = Modifiers(Modifiers::SUPER.0 | Modifiers::ALT.0);
        let resizing = Modifiers(Modifiers::SUPER.0 | Modifiers::CONTROL.0 | Modifiers::SHIFT.0);
        match self {
            Action::NewTab => Some(Chord::new(Key::KeyT, command)),
            // Ghostty's, and one finger away from next and previous pane - which is what they
            // are: the same walk, one level up. Muster's list crosses daemons where Ghostty's
            // cannot, but the gesture is the one somebody already has.
            Action::NextTab => Some(Chord::new(Key::BracketRight, shifted)),
            Action::PreviousTab => Some(Chord::new(Key::BracketLeft, shifted)),
            // ⌘1 to ⌘9, as every tabbed thing on this platform has them. The number is the
            // place in the window's tab order, so it counts across daemons the way the
            // sidebar does rather than restarting at each machine.
            Action::FocusTab(place) => Some(Chord::new(
                TAB_DIGITS.get(usize::from(place).wrapping_sub(1)).copied().unwrap_or(Key::Digit1),
                command,
            )),
            Action::SplitRight => Some(Chord::new(Key::KeyD, command)),
            Action::SplitDown => Some(Chord::new(Key::KeyD, shifted)),
            // Unbound, as Ghostty ships them. See `default_chord`.
            Action::SplitLeft | Action::SplitUp => None,
            Action::ClosePane => Some(Chord::new(Key::KeyW, command)),
            Action::NextPane => Some(Chord::new(Key::BracketRight, command)),
            Action::PreviousPane => Some(Chord::new(Key::BracketLeft, command)),
            Action::FocusLeft => Some(Chord::new(Key::ArrowLeft, optioned)),
            Action::FocusRight => Some(Chord::new(Key::ArrowRight, optioned)),
            Action::FocusUp => Some(Chord::new(Key::ArrowUp, optioned)),
            Action::FocusDown => Some(Chord::new(Key::ArrowDown, optioned)),
            Action::ResizeLeft => Some(Chord::new(Key::ArrowLeft, resizing)),
            Action::ResizeRight => Some(Chord::new(Key::ArrowRight, resizing)),
            Action::ResizeUp => Some(Chord::new(Key::ArrowUp, resizing)),
            Action::ResizeDown => Some(Chord::new(Key::ArrowDown, resizing)),
            Action::Zoom => Some(Chord::new(Key::Enter, shifted)),
            // The unshifted key rather than the plus printed above it. Ghostty binds both
            // because it lets an action carry several chords; Muster gives each one, and the
            // one to give is the one a hand actually makes.
            Action::IncreaseFontSize => Some(Chord::new(Key::Equal, command)),
            Action::DecreaseFontSize => Some(Chord::new(Key::Minus, command)),
            Action::ResetFontSize => Some(Chord::new(Key::Digit0, command)),
            // Ghostty has no sidebar, so this one comes from the wider platform instead:
            // ⌘B is what a Mac app with a list down the side puts it on, and no terminal
            // wants the chord for anything, since a command chord never reaches a pane.
            Action::ToggleSidebar => Some(Chord::new(Key::KeyB, command)),
            // Where a list of shortcuts lives in most things that have one. Nothing in a
            // terminal wants it, and it is the chord somebody presses when they are looking
            // for exactly this.
            Action::ShowShortcuts => Some(Chord::new(Key::Slash, command)),
        }
    }
}

/// What each numbered tab action is called, in place order.
const TAB_PLACES: [&str; 9] = [
    "focus_tab_1",
    "focus_tab_2",
    "focus_tab_3",
    "focus_tab_4",
    "focus_tab_5",
    "focus_tab_6",
    "focus_tab_7",
    "focus_tab_8",
    "focus_tab_9",
];

/// The digit key a numbered tab action sits on, in place order.
const TAB_DIGITS: [Key; 9] = [
    Key::Digit1,
    Key::Digit2,
    Key::Digit3,
    Key::Digit4,
    Key::Digit5,
    Key::Digit6,
    Key::Digit7,
    Key::Digit8,
    Key::Digit9,
];

/// A key under some modifiers, as a config file spells one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Chord {
    pub fn new(key: Key, modifiers: Modifiers) -> Chord {
        Chord { key, modifiers }
    }

    /// Reads `cmd+shift+d`, or says what it could not read.
    ///
    /// Split on `+`, modifiers in any order, the key last. Case is ignored, because somebody
    /// writing `Cmd+D` means the same thing and a config file that silently ignored it would
    /// be worse than one that said so.
    ///
    /// The refusal names the piece it could not read rather than the whole string. A config
    /// with one bad chord in twenty is a config where "invalid keybinding" is not an answer.
    pub fn parse(text: &str) -> Result<Chord, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a chord with nothing in it".to_string());
        }
        // Split on `+`, except a trailing one, which is how somebody spells the plus key.
        // The empty pieces that leaves behind are dropped rather than read, so `cmd++` is the
        // plus key under command rather than a chord with a hole in it.
        let mut parts: Vec<&str> =
            text.split('+').map(str::trim).filter(|part| !part.is_empty()).collect();
        if text.ends_with('+') {
            parts.push("+");
        }

        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for part in parts {
            if let Some(modifier) = modifier(&part.to_ascii_lowercase()) {
                modifiers = Modifiers(modifiers.0 | modifier.0);
                continue;
            }
            if key.is_some() {
                return Err(format!(
                    "`{part}` is a second key in one chord. A chord is any number of \
                     modifiers and exactly one key."
                ));
            }
            key = Some(named_key(part).ok_or_else(|| {
                format!(
                    "`{part}` is not a key Muster knows. Letters and digits are themselves, \
                     and the rest are spelled out: left, right, up, down, enter, tab, space, \
                     escape, backspace, delete, home, end, pageup, pagedown, f1 to f12."
                )
            })?);
        }

        match key {
            Some(key) => Ok(Chord::new(key, modifiers)),
            None => Err(format!("`{text}` is modifiers with no key")),
        }
    }
}

/// One modifier, under any of the names people write it with.
///
/// Both the platform's word and the standard's, because both are what somebody would type: a
/// Mac user writes `cmd` and `opt`, and a config carried over from elsewhere says `super` and
/// `alt`. Reading only one of each pair makes a config that looks right and does nothing.
fn modifier(name: &str) -> Option<Modifiers> {
    match name {
        "cmd" | "command" | "super" | "win" => Some(Modifiers::SUPER),
        "opt" | "option" | "alt" => Some(Modifiers::ALT),
        "ctrl" | "control" => Some(Modifiers::CONTROL),
        "shift" => Some(Modifiers::SHIFT),
        _ => None,
    }
}

/// One key, under the name a person writes rather than the one the wire uses.
///
/// `Key::parse` reads the W3C spelling - `KeyD`, `ArrowLeft` - which is right for a corpus
/// and wrong for a file somebody hand-edits. This is the friendly half, and it falls through
/// to the W3C name so both work.
fn named_key(name: &str) -> Option<Key> {
    let lowered = name.to_ascii_lowercase();
    let mut characters = lowered.chars();
    if let (Some(single), None) = (characters.next(), characters.next()) {
        if single.is_ascii_alphabetic() {
            return Key::parse(&format!("Key{}", single.to_ascii_uppercase()));
        }
        if single.is_ascii_digit() {
            return Key::parse(&format!("Digit{single}"));
        }
    }
    let spelled = match lowered.as_str() {
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "space" => "Space",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        ";" => "Semicolon",
        "'" => "Quote",
        "-" => "Minus",
        // Both spellings of one physical key: on a US layout the plus is printed above the
        // equals. Somebody with a numpad in mind writes NumpadAdd, which falls through to the
        // W3C name below.
        "=" | "+" => "Equal",
        "\\" => "Backslash",
        "`" => "Backquote",
        other => {
            // Function keys, and then the W3C spelling for anything else - so a name from the
            // corpus or from a log line is always readable here.
            if let Some(number) = other.strip_prefix('f')
                && number.parse::<u8>().is_ok_and(|n| (1..=25).contains(&n))
            {
                return Key::parse(&format!("F{number}"));
            }
            // The W3C name, and then the same name however it was capitalised. A config file
            // is hand-edited, so `backslash` and `arrowleft` are what somebody writes when the
            // friendly list above does not cover the key they want.
            return Key::parse(name).or_else(|| {
                Key::ALL.into_iter().find(|key| key.as_str().eq_ignore_ascii_case(name))
            });
        }
    };
    Key::parse(spelled)
}

/// Every action and the chord it is on: the defaults, with a config file's answers over them.
///
/// A partial config replaces the chords it names and leaves the rest, because that is what
/// somebody rebinding one key means. A file that had to restate all fifteen to change one
/// would be a file nobody edits twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    chords: BTreeMap<Action, Chord>,
}

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings {
            chords: Action::ALL
                .into_iter()
                .filter_map(|action| Some((action, action.default_chord()?)))
                .collect(),
        }
    }
}

impl Bindings {
    /// Every action and the chord it is on, in the order a menu lists them.
    ///
    /// Every action, including the ones on no chord, because a shell builds its menu from this
    /// and an action with no shortcut is still something a person can pick. Two ways one gets
    /// here and they want the same answer: somebody who unbound it wanted the chord back
    /// rather than the action gone, and two of the splits ship unbound because the terminal
    /// Muster embeds ships them that way. Dropping them here would make both invisible, and on
    /// macOS invisible means unreachable - the menu is the dispatch.
    pub fn all(&self) -> impl Iterator<Item = (Action, Option<Chord>)> + '_ {
        Action::ALL.into_iter().map(|action| (action, self.chords.get(&action).copied()))
    }

    pub fn chord(&self, action: Action) -> Option<Chord> {
        self.chords.get(&action).copied()
    }

    /// Rebinds one action, or says why it could not.
    pub fn bind(&mut self, action: Action, chord: &str) -> Result<(), String> {
        self.chords.insert(action, Chord::parse(chord)?);
        Ok(())
    }

    /// Unbinds an action outright.
    ///
    /// Worth having distinctly from "not mentioned in the config": somebody who wants ⌘W back
    /// for closing the window rather than the pane has to be able to say so, and the only
    /// alternative is binding it to a chord nobody presses.
    pub fn unbind(&mut self, action: Action) {
        self.chords.remove(&action);
    }
}
