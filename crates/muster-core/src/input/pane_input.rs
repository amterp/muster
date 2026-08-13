//! One pane's input path: keymap first, then encode, then out.
//!
//! The whole of "what happens when you type" in one place, so the shell above only has to
//! decide *that* a key was pressed and this decides what it means. It lives in the core
//! rather than beside the window because every decision here is testable and none of it is
//! about macOS - the two things it needs from the outside, an encoder and a channel, arrive
//! as traits.

use std::sync::{Arc, Mutex};

use super::{KeyEncoding, KeyEvent, Keymap, PaneChannel, PaneIntent, Resolution, ScrollDirection};
use crate::diagnostics::log;
use crate::fields;

pub struct PaneInput {
    channel: Arc<dyn PaneChannel>,
    server_channel: Option<Arc<dyn PaneChannel>>,
    encoder: Arc<dyn KeyEncoding>,
    keymap: Keymap,

    /// Everything leaves through here, in order.
    ///
    /// Two channels reach the same PTY by different routes: control-stream bytes travel app
    /// → bridge → daemon, while a server-encoded key goes app → daemon directly and skips a
    /// hop. Left concurrent, `abc<up>def` can deliver the arrow out of place. So sends are
    /// serialized and a server-encoded intent completes its round trip before the next item
    /// goes out - which costs nothing at typing speed and is what makes mixing the two
    /// routes safe at all.
    ///
    /// The one-shot warning lives inside the same lock because it is written on exactly the
    /// path this serializes.
    outbound: Mutex<Outbound>,
}

#[derive(Default)]
struct Outbound {
    warned_about_dropped_input: bool,
}

impl std::fmt::Debug for PaneInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneInput")
            .field("channel", &self.channel.description())
            .field("server_channel", &self.server_channel.as_ref().map(|c| c.description()))
            .finish_non_exhaustive()
    }
}

impl PaneInput {
    pub fn new(
        channel: Arc<dyn PaneChannel>,
        server_channel: Option<Arc<dyn PaneChannel>>,
        encoder: Arc<dyn KeyEncoding>,
        keymap: Keymap,
    ) -> PaneInput {
        PaneInput {
            channel,
            server_channel,
            encoder,
            keymap,
            outbound: Mutex::new(Outbound::default()),
        }
    }

    pub fn send(&self, key: &KeyEvent) {
        // Precedence: the keymap gets first refusal, and the encoder only sees what it
        // declines (architecture.md, input precedence).
        match self.keymap.resolve(key) {
            Resolution::Text(bytes) => {
                log::debug(
                    "input.bound.text",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                        "bytes" => bytes.len(),
                    },
                );
                self.deliver(&PaneIntent::Input(bytes));
                return;
            }
            Resolution::ServerEncoded(name) => {
                self.send_server_encoded(&name, key);
                return;
            }
            Resolution::Action(_) => {
                log::debug(
                    "input.bound",
                    fields! {
                        "key" => key.key.as_str(),
                        "mods" => key.modifiers.names().join("+"),
                    },
                );
                return;
            }
            Resolution::Unbound => {}
        }

        let Ok(bytes) = self.encoder.encode(key) else {
            log::warn(
                "input.encode.failed",
                fields! {
                    "key" => key.key.as_str(),
                    "mods" => key.modifiers.names().join("+"),
                    "impact" => "this keystroke reaches the pane as nothing at all",
                },
            );
            return;
        };
        // An empty encoding is normal and frequent - modifiers alone, and every key while an
        // input method is composing - so it is not a warning, but a silence worth being able
        // to tell apart from a dropped one.
        if bytes.is_empty() {
            log::trace(
                "input.key.empty",
                fields! { "key" => key.key.as_str(), "action" => key.action.as_str() },
            );
            return;
        }
        log::debug(
            "input.key",
            fields! {
                "key" => key.key.as_str(),
                "mods" => key.modifiers.names().join("+"),
                "action" => key.action.as_str(),
                "bytes" => bytes.len(),
                "encoded" => if log::includes_input() {
                    format!("{:?}", String::from_utf8_lossy(&bytes))
                } else {
                    String::new()
                },
            },
        );
        self.deliver(&PaneIntent::Input(bytes));
    }

    pub fn send_text(&self, text: &str) {
        log::debug(
            "input.text",
            fields! {
                "characters" => text.chars().count(),
                "text" => if log::includes_input() { format!("{text:?}") } else { String::new() },
            },
        );
        self.deliver(&PaneIntent::Input(text.as_bytes().to_vec()));
    }

    /// Sends the clipboard to the pane.
    ///
    /// Server-encoded when there is a channel that can: a program which enabled DEC 2004
    /// wants the text fenced by paste markers so it can tell pasting from very fast typing,
    /// and a shell uses the same fence to stop a multi-line paste running as it arrives.
    /// Only the daemon knows whether that mode is on. A paste is one action rather than one
    /// per keystroke, so the round trip it costs is free.
    ///
    /// Without such a channel the text goes raw and unfenced, which is right for a single
    /// line and wrong for several. Guessing the fence on would be worse: markers sent to a
    /// program that never asked arrive as literal `[200~` on its input.
    pub fn paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        log::info(
            "input.paste",
            fields! {
                "characters" => text.chars().count(),
                "server_encoded" => self.server_channel.is_some(),
                "text" => if log::includes_input() { format!("{text:?}") } else { String::new() },
            },
        );
        let Some(server) = self.server_channel.clone() else {
            self.deliver(&PaneIntent::Input(text.as_bytes().to_vec()));
            return;
        };
        self.deliver_over(
            &PaneIntent::Text(text.to_string()),
            server.as_ref(),
            Some(&PaneIntent::Input(text.as_bytes().to_vec())),
        );
    }

    pub fn scroll(&self, direction: ScrollDirection, lines: u16) {
        self.deliver(&PaneIntent::Scroll { direction, lines });
    }

    /// Hands a key to the daemon to encode, because we would get it wrong.
    ///
    /// Falls back to local encoding rather than dropping the key: a guessed arrow beats no
    /// arrow, and a daemon that has gone away must not take the keyboard with it.
    fn send_server_encoded(&self, name: &str, key: &KeyEvent) {
        let Some(server) = self.server_channel.clone() else {
            self.send_locally_encoded(key);
            return;
        };
        log::debug("input.key.server", fields! { "key" => key.key.as_str(), "name" => name });
        let local = self.encoder.encode(key).unwrap_or_default();
        self.deliver_over(
            &PaneIntent::Key { name: name.to_string() },
            server.as_ref(),
            Some(&PaneIntent::Input(local)),
        );
    }

    fn send_locally_encoded(&self, key: &KeyEvent) {
        match self.encoder.encode(key) {
            Ok(bytes) if !bytes.is_empty() => self.deliver(&PaneIntent::Input(bytes)),
            _ => {}
        }
    }

    fn deliver(&self, intent: &PaneIntent) {
        self.deliver_over(intent, self.channel.clone().as_ref(), None);
    }

    fn deliver_over(
        &self,
        intent: &PaneIntent,
        target: &dyn PaneChannel,
        fallback: Option<&PaneIntent>,
    ) {
        let mut outbound = self.outbound.lock().expect("a panicking sender poisoned the lock");
        if target.deliver(intent) {
            return;
        }
        if let Some(fallback) = fallback.filter(|f| *f != intent) {
            log::warn(
                "input.fallback",
                fields! {
                    "channel" => target.description(),
                    "impact" => "sent with a guessed encoding instead, which may be wrong for this pane",
                },
            );
            if self.channel.deliver(fallback) {
                return;
            }
        }
        report_dropped(target, &mut outbound);
    }
}

fn report_dropped(target: &dyn PaneChannel, outbound: &mut Outbound) {
    log::warn(
        "input.dropped",
        fields! {
            "channel" => target.description(),
            "impact" => "the pane looks frozen but is fine; nothing typed here reached it",
        },
    );
    // Once on stderr, not per keystroke: a pane that swallows input produces a lot of them,
    // and a log that scrolls is a log nobody reads. The record above is per event.
    if outbound.warned_about_dropped_input {
        return;
    }
    outbound.warned_about_dropped_input = true;
    eprint!(
        "muster: the pane bridge is not connected, so input is going nowhere.\n\
         The pane keeps rendering, which makes this look like a frozen program rather than a \
         broken channel. Usual causes: muster-bridge failed to start (its own error is above), \
         or it could not reach {}.\n\n",
        target.description()
    );
}
