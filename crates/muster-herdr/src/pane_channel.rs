//! Where Muster's vocabulary becomes herdr's.
//!
//! The core produces `PaneIntent`; everything that knows what a `terminal.input` envelope
//! looks like lives on this side of the line (architecture.md, the backend seam).
//!
//! Two channels reach the same PTY, and they differ in what they can express. The control
//! stream is a raw pipe, so it carries bytes we encoded ourselves and a scroll the daemon
//! answers. The daemon's own API encodes against the pane's live modes, which is the only
//! way to get an arrow or a paste right - and costs a round trip, which is why it carries
//! the exceptions rather than everything.

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{PaneChannel, PaneIntent};
use serde_json::json;

use crate::client::HerdrClient;
use crate::control_socket::PaneControlChannel;
use crate::control_stream::ControlStreamMessage;

impl ControlStreamMessage {
    /// This intent as a control-stream message, or `None` if the stream cannot express it.
    ///
    /// Text and named keys are exactly the intents that need the pane's real modes to
    /// encode, which is the thing this channel does not have.
    pub fn from_intent(intent: &PaneIntent) -> Option<ControlStreamMessage> {
        match intent {
            PaneIntent::Input(bytes) => Some(ControlStreamMessage::Input(bytes.clone())),
            PaneIntent::Scroll { direction, lines } => {
                Some(ControlStreamMessage::Scroll { direction: *direction, lines: *lines })
            }
            PaneIntent::Text(_) | PaneIntent::Key { .. } => None,
        }
    }
}

impl PaneChannel for PaneControlChannel {
    fn deliver(&self, intent: &PaneIntent) -> bool {
        match ControlStreamMessage::from_intent(intent) {
            Some(message) => self.send(&message),
            None => false,
        }
    }

    /// No. Every byte on this channel was encoded by us, against a guess.
    fn encodes_server_side(&self) -> bool {
        false
    }

    fn description(&self) -> &str {
        self.socket_path()
    }
}

/// The channel for input Muster cannot encode correctly by itself.
///
/// herdr's `pane.send_input` encodes against the pane's live terminal state - the kitty
/// flags it has negotiated, whether it turned on application cursor keys, whether bracketed
/// paste is enabled - none of which is visible to a control-stream client
/// (`docs/observations/herdr-0.8.0.md` section 5). So the keys and text where guessing is
/// known to go wrong come here instead, and the daemon gets them right.
#[derive(Debug)]
pub struct HerdrPaneChannel {
    client: HerdrClient,
    pane_id: String,
    description: String,
}

impl HerdrPaneChannel {
    /// Returns `None` when no daemon socket can be found, which is a real state rather than
    /// an error - a pane still works without this channel, with a guessed encoding.
    pub fn discover(pane_id: impl Into<String>) -> Option<HerdrPaneChannel> {
        let socket = crate::discover_socket_path(&std::env::vars().collect())?;
        Some(HerdrPaneChannel::new(HerdrClient::new(socket), pane_id))
    }

    pub fn new(client: HerdrClient, pane_id: impl Into<String>) -> HerdrPaneChannel {
        let pane_id = pane_id.into();
        let description = format!("herdr {} ({})", client.socket_path(), pane_id);
        log::info(
            "server_channel.ready",
            fields! { "pane" => &pane_id, "socket" => client.socket_path() },
        );
        HerdrPaneChannel { client, pane_id, description }
    }
}

impl PaneChannel for HerdrPaneChannel {
    fn deliver(&self, intent: &PaneIntent) -> bool {
        let params = match intent {
            PaneIntent::Text(text) => json!({ "pane_id": self.pane_id, "text": text }),
            PaneIntent::Key { name } => json!({ "pane_id": self.pane_id, "keys": [name] }),
            // Not this channel's job. Bytes already encoded belong on the control stream,
            // and a scroll is answered there against the same live state.
            PaneIntent::Input(_) | PaneIntent::Scroll { .. } => return false,
        };

        let started = std::time::Instant::now();
        let result = self.client.request("pane.send_input", &params);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(_) => {
                // Timed on every send because this sits on the input path, and the decision
                // to route a key this way rests on the number being small (card
                // a_26BIX28HG).
                log::debug(
                    "server_channel.sent",
                    fields! { "intent" => label(intent), "ms" => format!("{elapsed_ms:.2}") },
                );
                true
            }
            Err(failure) => {
                log::warn(
                    "server_channel.failed",
                    fields! {
                        "intent" => label(intent),
                        "error" => failure.to_string(),
                        "ms" => format!("{elapsed_ms:.2}"),
                        "impact" => "falls back to a locally guessed encoding, which pagers reject",
                    },
                );
                false
            }
        }
    }

    fn encodes_server_side(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        &self.description
    }
}

fn label(intent: &PaneIntent) -> String {
    match intent {
        PaneIntent::Key { name } => format!("key:{name}"),
        PaneIntent::Text(_) => "text".to_string(),
        PaneIntent::Input(_) => "input".to_string(),
        PaneIntent::Scroll { .. } => "scroll".to_string(),
    }
}
