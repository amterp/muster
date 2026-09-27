//! One pane's input path: keymap first, then out to the daemon.
//!
//! The whole of "what happens when you type" in one place, so the shell above only has to
//! decide *that* a key was pressed and this decides what it means. It lives in the core
//! rather than beside the window because every decision here is testable and none of it is
//! about macOS - what it needs from the outside, a way to reach the pane's daemon, arrives as
//! a trait.

use std::sync::{Arc, RwLock};

use super::{InputEvent, InputSink, KeyEvent, Keymap, PaneInputSettings, Resolution};
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

    /// Sends one keystroke, unless the keymap takes it.
    pub fn send(&self, key: &KeyEvent) {
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
            }
            Resolution::Action(_) => {
                log::debug(
                    "input.bound",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                    },
                );
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

    fn deliver(&self, event: InputEvent) {
        self.sink.send(&self.pane, event);
        if let Some(delivered) = self.delivered.as_ref() {
            delivered();
        }
    }
}
