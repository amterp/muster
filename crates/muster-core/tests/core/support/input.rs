//! A fake of what the input path sends to.
//!
//! Part of the contract, not incidental scaffolding: `pane-input.json` states what the input
//! path sends, and this is where it is recorded, so a driver in any language builds the same
//! one.

use std::sync::Mutex;

use muster_core::input::{InputEvent, InputSink};
use muster_core::mirror::backend::PaneId;

/// Every event sent, in order, with the pane it was for.
#[derive(Debug, Default)]
pub(crate) struct RecordingSink {
    sent: Mutex<Vec<(PaneId, InputEvent)>>,
}

impl RecordingSink {
    pub(crate) fn sent(&self) -> Vec<(PaneId, InputEvent)> {
        self.sent.lock().expect("a panicking sender poisoned the recorder").clone()
    }
}

impl InputSink for RecordingSink {
    fn send(&self, pane: &PaneId, event: InputEvent) {
        self.sent
            .lock()
            .expect("a panicking sender poisoned the recorder")
            .push((pane.clone(), event));
    }

    fn description(&self) -> &'static str {
        "a recording"
    }
}
