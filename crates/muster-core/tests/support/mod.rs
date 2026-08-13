//! Fakes the input path needs, shared by the drivers that need them.
//!
//! These are part of the contract, not incidental scaffolding: `pane-input.json` states
//! what they do, so a driver in any language builds the same ones. A fake that spelled a
//! keystroke differently would make every case disagree for a reason unrelated to the
//! behavior under test.

use std::sync::{Arc, Mutex};

use muster_core::input::{EncodeError, KeyEncoding, KeyEvent, PaneChannel, PaneIntent};

/// One ordered account of everything the input path sent, across every channel.
///
/// Shared rather than per-channel because the interesting property spans them: bytes go out
/// over a control stream while named keys go straight to the daemon, and the question is
/// whether `abc<up>def` still arrives in that order. Two separate logs could not say.
#[derive(Debug, Default)]
pub(crate) struct SendRecorder {
    sends: Mutex<Vec<(String, PaneIntent)>>,
}

impl SendRecorder {
    pub(crate) fn record(&self, channel: &str, intent: &PaneIntent) {
        let mut sends = self.sends.lock().expect("a panicking sender poisoned the recorder");
        sends.push((channel.to_string(), intent.clone()));
    }

    pub(crate) fn sends(&self) -> Vec<(String, PaneIntent)> {
        self.sends.lock().expect("a panicking sender poisoned the recorder").clone()
    }
}

/// A pane channel that writes to a recorder instead of a daemon.
#[derive(Debug)]
pub(crate) struct FakeChannel {
    name: String,
    recorder: Arc<SendRecorder>,
    encodes_server_side: bool,
    /// Whether sends succeed. Refusal is a real state worth faking: the daemon channel
    /// refuses anything it cannot encode, and any channel can refuse once the far end is
    /// gone.
    accepts: bool,
}

impl FakeChannel {
    pub(crate) fn new(
        name: &str,
        recorder: Arc<SendRecorder>,
        encodes_server_side: bool,
        accepts: bool,
    ) -> FakeChannel {
        FakeChannel { name: name.to_string(), recorder, encodes_server_side, accepts }
    }
}

impl PaneChannel for FakeChannel {
    fn deliver(&self, intent: &PaneIntent) -> bool {
        if !self.accepts {
            return false;
        }
        self.recorder.record(&self.name, intent);
        true
    }

    fn encodes_server_side(&self) -> bool {
        self.encodes_server_side
    }

    fn description(&self) -> &str {
        &self.name
    }
}

/// An encoder that spells a keystroke the obvious way.
///
/// Enough for testing what the pipeline *does* with bytes. What the bytes should be is a
/// separate question, answered in key-encoder.json against libghostty's own output rather
/// than against a fixture written from memory.
#[derive(Debug, Default)]
pub(crate) struct FakeEncoder;

impl KeyEncoding for FakeEncoder {
    fn encode(&self, key: &KeyEvent) -> Result<Vec<u8>, EncodeError> {
        Ok(key.text.as_bytes().to_vec())
    }
}
