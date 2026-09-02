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
    /// Opens another window, which means another Muster.
    ///
    /// The one action here the core does not carry out. A window is a process - the session is
    /// a global, one per process - so making one is starting an app, which is an OS act the
    /// shell performs itself. It is an action rather than a plain menu item so that it can be
    /// rebound and unbound like everything else, on the same terms as `show_shortcuts`, which
    /// the shell also answers alone.
    NewWindow,
    /// Brings back the window that was closed, which is the same act with one flag off.
    ///
    /// The other action the core does not carry out, on the same terms as `NewWindow`: a window
    /// is a process, so this is the shell starting an app. What it starts it with is the
    /// difference - a window somebody asked for takes an arrangement nothing has ever held, and
    /// this takes the most recent one no live window is holding.
    ReopenWindow,

    /// Quits, and ends the sessions this window is attached to on the way out.
    ///
    /// The destructive half of a choice that until now had only one side and made it silently.
    /// Quitting leaves every daemon running, forever, which is the founding promise and stays
    /// the default; what was missing is that somebody finished for the day had no way to say so
    /// (kan a_28YghIUw2).
    ///
    /// Answered by the shell, like `new_window` and for the same reason: ending the app is an
    /// OS act. What the shell cannot do alone is end the daemons - only the core knows which
    /// sockets they are on - so it asks the core before it goes.
    ///
    /// Unbound by default, deliberately. Every other action here is recoverable by doing it
    /// again; this one ends processes holding somebody's work, and a chord it shipped on is a
    /// chord somebody presses by accident.
    QuitAndCloseSessions,
    NewTab,
    NextTab,
    PreviousTab,
    SplitRight,
    SplitDown,
    SplitLeft,
    SplitUp,
    /// Asks the shell for a name for a pane, and renames it to whatever comes back.
    ///
    /// The one action here that needs something from the person before it can be dispatched,
    /// which is why the name is not part of it: what a chord means is "ask me", and the answer
    /// arrives as an ordinary rename request afterwards. A CLI naming a pane outright sends
    /// that request and never this.
    RenamePane,
    RenameTab,
    /// Closes the tab the keyboard is in, and every pane in it.
    ///
    /// The tab half of `ClosePane`, and it ends more than it names. Unbound for that reason
    /// among others: a chord that destroys several panes is one somebody reaches by accident.
    CloseTab,
    /// Takes the pane the keyboard is on out of its split and gives it a tab of its own.
    ///
    /// The only half of moving a pane that an action can carry. Moving one *beside* another
    /// names two panes, and nothing about a chord says which second pane - that is why dragging
    /// a row exists and why it is not in this list. This one names nowhere, so a chord and a
    /// menu item can mean it.
    MovePaneToNewTab,
    ClosePane,
    NextPane,
    PreviousPane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    /// Puts the keyboard on the pane at this place in the window's pane order, counting from
    /// one - the place the sidebar draws beside the row.
    ///
    /// One variant carrying a number rather than nine spelled out, because they differ only
    /// by the digit and a list of nine near-identical arms is a list nobody keeps in step.
    /// Nine names in the config file over one intent here is the general rule: the file names
    /// menu items and the core names intents (`architecture.md`, one action path).
    ///
    /// Only 1 to 9 are ever built - [`Action::ALL`] is the whole vocabulary - so the places
    /// beyond that have no name, no chord and no menu item. A tenth pane is reached by
    /// `next_pane`, by a direction, or by clicking its row.
    FocusPane(u8),
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    /// Opens the find bar over the pane with the keyboard.
    ///
    /// Like [`Action::RenamePane`], what the chord means is "ask me": the needle arrives
    /// afterwards as an ordinary find request, one per keystroke, and a CLI searching for
    /// something outright sends that and never this.
    Find,
    /// Goes to the next match, and to the previous one.
    ///
    /// Next climbs the pane, because the first match is the newest and the ones before it are
    /// further up. Actions rather than keys the find bar swallows, so that they are rebindable
    /// and reachable from the menu like everything else Muster does - and because a person
    /// walking matches usually has the keyboard in the pane rather than in the bar.
    FindNext,
    FindPrevious,
    Zoom,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ToggleSidebar,
    ReloadConfig,
    ShowShortcuts,
}

impl Action {
    /// Every action, which is also the order a menu lists them in.
    ///
    /// Deliberately not alphabetical: a menu is read top to bottom, and the order here is what
    /// somebody scanning it expects - making something, then arranging it, then moving around
    /// it. A shell that sorted these would produce a menu nobody can find anything in.
    pub const ALL: [Action; 44] = [
        Action::NewWindow,
        Action::ReopenWindow,
        Action::NewTab,
        Action::NextTab,
        Action::PreviousTab,
        Action::RenameTab,
        Action::CloseTab,
        Action::SplitRight,
        Action::SplitDown,
        Action::SplitLeft,
        Action::SplitUp,
        Action::RenamePane,
        Action::MovePaneToNewTab,
        Action::ClosePane,
        Action::NextPane,
        Action::PreviousPane,
        Action::FocusLeft,
        Action::FocusRight,
        Action::FocusUp,
        Action::FocusDown,
        Action::FocusPane(1),
        Action::FocusPane(2),
        Action::FocusPane(3),
        Action::FocusPane(4),
        Action::FocusPane(5),
        Action::FocusPane(6),
        Action::FocusPane(7),
        Action::FocusPane(8),
        Action::FocusPane(9),
        Action::ResizeLeft,
        Action::ResizeRight,
        Action::ResizeUp,
        Action::ResizeDown,
        Action::Find,
        Action::FindNext,
        Action::FindPrevious,
        Action::Zoom,
        Action::IncreaseFontSize,
        Action::DecreaseFontSize,
        Action::ResetFontSize,
        Action::ToggleSidebar,
        Action::ReloadConfig,
        Action::ShowShortcuts,
        Action::QuitAndCloseSessions,
    ];

    /// The name a config file, a log line and the seam all spell it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::ReopenWindow => "reopen_window",
            Action::NewTab => "new_tab",
            Action::NextTab => "next_tab",
            Action::PreviousTab => "previous_tab",
            Action::SplitRight => "split_right",
            Action::SplitDown => "split_down",
            Action::SplitLeft => "split_left",
            Action::SplitUp => "split_up",
            Action::RenamePane => "rename_pane",
            Action::RenameTab => "rename_tab",
            Action::CloseTab => "close_tab",
            Action::MovePaneToNewTab => "move_pane_to_new_tab",
            Action::ClosePane => "close_pane",
            Action::NextPane => "next_pane",
            Action::PreviousPane => "previous_pane",
            Action::FocusLeft => "focus_left",
            Action::FocusRight => "focus_right",
            Action::FocusUp => "focus_up",
            Action::FocusDown => "focus_down",
            // A table rather than a format, because the answer is a `&'static str` and a name
            // built at a call site cannot be one. A place outside it comes back unnameable
            // rather than borrowing another place's name, so `parse` refuses it and nothing
            // silently binds ⌘4 to the wrong pane.
            Action::FocusPane(place) => {
                PANE_PLACES.get(usize::from(place).wrapping_sub(1)).copied().unwrap_or("focus_pane")
            }
            Action::ResizeLeft => "resize_left",
            Action::ResizeRight => "resize_right",
            Action::ResizeUp => "resize_up",
            Action::ResizeDown => "resize_down",
            Action::Find => "find",
            Action::FindNext => "find_next",
            Action::FindPrevious => "find_previous",
            Action::Zoom => "zoom",
            Action::IncreaseFontSize => "increase_font_size",
            Action::DecreaseFontSize => "decrease_font_size",
            Action::ResetFontSize => "reset_font_size",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::ReloadConfig => "reload_config",
            Action::NewWindow => "new_window",
            Action::ShowShortcuts => "show_shortcuts",
            Action::QuitAndCloseSessions => "quit_and_close_sessions",
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
            // Ghostty's, and every other macOS app's. It was the one chord in that set
            // Muster had nothing to put behind it.
            Action::NewWindow => Some(Chord::new(Key::KeyN, command)),
            Action::NewTab => Some(Chord::new(Key::KeyT, command)),
            // Ghostty's, and one finger away from next and previous pane - which is what they
            // are: the same walk, one level up. Muster's list crosses daemons where Ghostty's
            // cannot, but the gesture is the one somebody already has.
            Action::NextTab => Some(Chord::new(Key::BracketRight, shifted)),
            Action::PreviousTab => Some(Chord::new(Key::BracketLeft, shifted)),
            Action::SplitRight => Some(Chord::new(Key::KeyD, command)),
            Action::SplitDown => Some(Chord::new(Key::KeyD, shifted)),
            // Unbound, each for its own reason. The two splits are Ghostty parity - it ships
            // `new_split:left` and `new_split:up` with no chord, so Muster invents none
            // either. Renaming a tab has no Ghostty equivalent at all, and is something done
            // once per tab where renaming a pane is done several times an hour: the chord
            // goes to the common one and the menu carries this. Pulling a pane into a tab of
            // its own is the same shape as renaming a tab and has the same answer, and it is
            // also the newest of the three - a chord invented for it would be one nobody
            // asked for. Ending the sessions is the odd one out and is unbound for safety
            // rather than for parity: every other action here is undone by doing it again,
            // and that one ends processes holding somebody's work. `[keymap]` is one line
            // away for anybody who disagrees with any of them.
            Action::SplitLeft
            | Action::SplitUp
            | Action::RenameTab
            | Action::MovePaneToNewTab
            | Action::CloseTab
            | Action::ReopenWindow
            | Action::QuitAndCloseSessions => None,
            // Muster's own, since Ghostty has no equivalent. ⌘⇧N is free on this platform for
            // a terminal - a command chord never reaches a pane - and naming panes is what
            // somebody does in a window of fifteen agents, which is the size this was built
            // for.
            Action::RenamePane => Some(Chord::new(Key::KeyN, shifted)),
            Action::ClosePane => Some(Chord::new(Key::KeyW, command)),
            Action::NextPane => Some(Chord::new(Key::BracketRight, command)),
            Action::PreviousPane => Some(Chord::new(Key::BracketLeft, command)),
            Action::FocusLeft => Some(Chord::new(Key::ArrowLeft, optioned)),
            Action::FocusRight => Some(Chord::new(Key::ArrowRight, optioned)),
            Action::FocusUp => Some(Chord::new(Key::ArrowUp, optioned)),
            Action::FocusDown => Some(Chord::new(Key::ArrowDown, optioned)),
            // ⌘1 to ⌘9, where every tabbed application on this platform puts them - pointed at
            // panes rather than tabs, because an agent is a pane and the rows carrying the
            // agent states are pane rows. The number counts across daemons the way the sidebar
            // does rather than restarting at each machine.
            Action::FocusPane(place) => Some(Chord::new(
                PANE_DIGITS.get(usize::from(place).wrapping_sub(1)).copied().unwrap_or(Key::Digit1),
                command,
            )),
            Action::ResizeLeft => Some(Chord::new(Key::ArrowLeft, resizing)),
            Action::ResizeRight => Some(Chord::new(Key::ArrowRight, resizing)),
            Action::ResizeUp => Some(Chord::new(Key::ArrowUp, resizing)),
            Action::ResizeDown => Some(Chord::new(Key::ArrowDown, resizing)),
            // Ghostty's, and the platform's: ⌘F opens a find and ⌘G walks it in every Mac app
            // that has one. Ghostty spells the walk `navigate_search:next|previous` on exactly
            // these chords, so somebody arriving from it needs nothing new.
            Action::Find => Some(Chord::new(Key::KeyF, command)),
            Action::FindNext => Some(Chord::new(Key::KeyG, command)),
            Action::FindPrevious => Some(Chord::new(Key::KeyG, shifted)),
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
            // Ghostty's own, and one of the few chords it has that Muster's config file needs
            // for the same job.
            Action::ReloadConfig => Some(Chord::new(Key::Comma, shifted)),
            // Where a list of shortcuts lives in most things that have one. Nothing in a
            // terminal wants it, and it is the chord somebody presses when they are looking
            // for exactly this.
            Action::ShowShortcuts => Some(Chord::new(Key::Slash, command)),
        }
    }
}

/// What each numbered pane action is called, in place order.
///
/// These used to be `focus_tab_1` through `focus_tab_9`, and the old names are gone rather
/// than aliased: an action `[keymap]` does not know refuses the whole file and says so, which
/// is what a config carried over from before should get. Silently binding ⌘3 to a different
/// thing than it used to reach would be the one outcome worse than the refusal.
const PANE_PLACES: [&str; 9] = [
    "focus_pane_1",
    "focus_pane_2",
    "focus_pane_3",
    "focus_pane_4",
    "focus_pane_5",
    "focus_pane_6",
    "focus_pane_7",
    "focus_pane_8",
    "focus_pane_9",
];

/// What ⌘1 to ⌘9 name: panes, or a tab and then a pane inside it.
///
/// **A prototype sits beside the settled answer here, and may not survive.** `Panes` is what
/// Muster does and what the whole argument in the README is for - one flat count down the
/// agent list, so ⌘3 is the third row whichever machine holds it. `TabThenPane` is the other
/// shape, being tried: ⌘2 goes to the second tab, and the ⌘2 after it goes to that tab's
/// second pane.
///
/// The two coexist rather than one replacing the other because the question is how the
/// second one feels after a day, and that cannot be answered by reading. Deleting the
/// prototype is deleting the second variant, and then this type.
///
/// Note what does *not* change with it: the nine actions, their names, their chords and
/// their menu items are the same either way. A scheme that added nine more actions on the
/// same nine chords would be refused by Muster's own collision rule, and one that spelled
/// out tab-by-pane would be eighty-one names. So the chord stays put and its meaning moves,
/// which is also why `focus_pane_3` reads as a small lie under the prototype - a cost taken
/// deliberately, because renaming nine actions is what would make this expensive to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumberedChords {
    #[default]
    Panes,
    TabThenPane,
}

impl NumberedChords {
    pub fn parse(name: &str) -> Option<NumberedChords> {
        match name {
            "panes" => Some(NumberedChords::Panes),
            "tab_then_pane" => Some(NumberedChords::TabThenPane),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NumberedChords::Panes => "panes",
            NumberedChords::TabThenPane => "tab_then_pane",
        }
    }

    /// Every value, for a refusal that has to say what was allowed.
    pub const READABLE: [&'static str; 2] = ["panes", "tab_then_pane"];
}

/// The digit key a numbered pane action sits on, in place order.
const PANE_DIGITS: [Key; 9] = [
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
