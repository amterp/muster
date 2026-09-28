//! One pane's input path: keymap first, then out to the daemon.
//!
//! The whole of "what happens when you type" in one place, so the shell above only has to
//! decide *that* a key was pressed and this decides what it means. It lives in the core
//! rather than beside the window because every decision here is testable and none of it is
//! about macOS - what it needs from the outside, a way to reach the pane's daemon, arrives as
//! a trait.

use std::sync::{Arc, RwLock};

use super::{InputEvent, InputSink, KeyEvent, Keymap, NotSent, PaneInputSettings, Resolution};
use crate::diagnostics::log;
use crate::fields;
use crate::mirror::backend::PaneId;

/// The input path into one pane.
pub struct PaneInput {
    pane: PaneId,
    sink: Arc<dyn InputSink>,
    typing: RwLock<Typing>,
    /// Told whenever something reaches the pane, so that a pane asked for something and
    /// painting nothing can be noticed (`crate::painting`).
    delivered: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Typing {
    keymap: Keymap,
    settings: PaneInputSettings,
}

impl std::fmt::Debug for PaneInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneInput")
            .field("pane", &self.pane)
            .field("sink", &self.sink.description())
            .finish_non_exhaustive()
    }
}

impl PaneInput {
    pub fn new(pane: PaneId, sink: Arc<dyn InputSink>, settings: &PaneInputSettings) -> PaneInput {
        PaneInput {
            pane,
            sink,
            typing: RwLock::new(Typing { keymap: settings.keymap(), settings: settings.clone() }),
            delivered: None,
        }
    }

    #[must_use]
    pub fn delivering_to(mut self, watcher: Arc<dyn Fn() + Send + Sync>) -> PaneInput {
        self.delivered = Some(watcher);
        self
    }

    /// Takes a reloaded config's settings for keystrokes from now on.
    pub fn resettle(&self, settings: &PaneInputSettings) {
        let mut typing = self.typing.write().expect("a panicking sender poisoned the settings");
        *typing = Typing { keymap: settings.keymap(), settings: settings.clone() };
    }

    /// Sends one keystroke, unless the keymap takes it, and says whether the pane got it: as
    /// itself, or as the text a binding writes.
    pub fn send(&self, key: &KeyEvent) -> bool {
        let (resolution, as_alt, option_as_alt) = {
            let typing = self.typing.read().expect("a panicking sender poisoned the settings");
            (typing.keymap.resolve(key), typing.settings.as_alt(key), typing.settings.option_as_alt)
        };
        match resolution {
            Resolution::Text(bytes) => {
                log::debug(
                    "input.bound.text",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                        "bytes" => bytes.len(),
                    },
                );
                self.deliver(InputEvent::Bytes(bytes));
                true
            }
            Resolution::Action(_) => {
                log::debug(
                    "input.bound",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                    },
                );
                false
            }
            Resolution::Unbound => {
                let key = as_alt.unwrap_or_else(|| key.clone());
                log::debug(
                    "input.key",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                        "action" => key.action.as_str(),
                    },
                );
                self.deliver(InputEvent::Key { key, option_as_alt });
                true
            }
        }
    }

    /// Text an input method committed, written as it stands: neither a keystroke for the
    /// encoder to reinterpret nor a paste to fence, which is what Ghostty's own surface does
    /// with a commit.
    pub fn send_text(&self, text: &str) {
        log::debug(
            "input.text",
            fields! {
                "characters" => text.chars().count(),
                "text" => if log::includes_input() { format!("{text:?}") } else { String::new() },
            },
        );
        self.deliver(InputEvent::Bytes(text.as_bytes().to_vec()));
    }

    /// A paste, which the daemon holds for confirmation when writing it would run several
    /// lines as typed. `confirmed` is somebody saying yes to one it held.
    pub fn paste(&self, text: &str, confirmed: bool) {
        if text.is_empty() {
            return;
        }
        log::info(
            "input.paste",
            fields! {
                "characters" => text.chars().count(),
                "confirmed" => confirmed,
                "text" => if log::includes_input() { format!("{text:?}") } else { String::new() },
            },
        );
        self.deliver(InputEvent::Paste { text: text.to_string(), confirmed });
    }

    /// A wheel turn or the mouse over the pane, for a program that may have asked for it.
    ///
    /// Not counted as something reaching the pane: most programs draw nothing for a wheel or a
    /// click, and a pane that stays still after one is not frozen.
    pub fn pointer(&self, event: InputEvent) {
        self.send_uncounted(event);
    }

    /// Ghostty's clear_screen, with the key whose binding asked for it if one did.
    ///
    /// Not counted: on the alternate screen nothing is cleared, and a program handed the key
    /// may draw nothing for it.
    pub fn clear_screen(&self, key: Option<&KeyEvent>) {
        let (key, option_as_alt) = {
            let typing = self.typing.read().expect("a panicking sender poisoned the settings");
            let key = key.map(|key| typing.settings.as_alt(key).unwrap_or_else(|| key.clone()));
            (key, typing.settings.option_as_alt)
        };
        log::info("input.clear_screen", fields! { "pane" => self.pane.to_string() });
        self.send_uncounted(InputEvent::ClearScreen { key, option_as_alt });
    }

    /// Ghostty's reset, which repaints the pane from a blank terminal, so it is counted.
    pub fn reset(&self) {
        log::info("input.reset", fields! { "pane" => self.pane.to_string() });
        self.deliver(InputEvent::Reset);
    }

    /// The pane gained or lost the keyboard of a focused window, for a program that asked to
    /// hear it. Not counted either: a program that never asked draws nothing for it.
    pub fn focus(&self, focused: bool) {
        self.send_uncounted(InputEvent::Focus(focused));
    }

    fn send_uncounted(&self, event: InputEvent) {
        if let Err(not_sent) = self.sink.send(&self.pane, event) {
            log::debug(
                "input.not_sent",
                fields! { "pane" => self.pane.to_string(), "why" => not_sent.to_string() },
            );
        }
    }

    fn deliver(&self, event: InputEvent) {
        match self.sink.send(&self.pane, event) {
            Ok(()) => {
                if let Some(delivered) = self.delivered.as_ref() {
                    delivered();
                }
            }
            // Only a paste can be this large, and nothing else says it was dropped.
            Err(not_sent @ NotSent::TooLarge { .. }) => log::warn(
                "input.not_sent",
                fields! {
                    "pane" => self.pane.to_string(),
                    "why" => not_sent.to_string(),
                    "impact" => "the paste did not reach the pane; nothing of it was typed",
                    "check" => "paste it in parts, or put it in a file and give the program \
                                the file's path",
                },
            ),
            // The connection says why once, where it ended or stalled, rather than per key.
            Err(not_sent) => log::debug(
                "input.not_sent",
                fields! { "pane" => self.pane.to_string(), "why" => not_sent.to_string() },
            ),
        }
    }
}
