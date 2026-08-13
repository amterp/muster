//! Muster's requested changes, in herdr's words.
//!
//! The write half of the adapter, and the mirror image of `events.rs`: everything that knows
//! what a `pane.split` envelope looks like is here, so a second backend is a second file
//! rather than a search for every place a JSON key leaked.
//!
//! Two translations are worth naming. A split's axis becomes the direction herdr names it
//! for - a column puts the new pane to the `right`, a row puts it `down` - which is the same
//! translation `layout.rs` does in reverse when it reads a tree back. And a path down the
//! tree becomes an array of booleans, herdr's own spelling for the turns.

use muster_core::intent::{BackendChannel, BackendIntent, Branch};
use muster_core::mirror::backend::SplitAxis;
use serde_json::{Value, json};

use crate::client::HerdrClient;

impl BackendChannel for HerdrClient {
    fn submit(&self, intent: &BackendIntent) -> Result<(), String> {
        let (method, params) = request(intent);
        self.request(method, &params).map(|_| ()).map_err(|failure| failure.to_string())
    }

    fn description(&self) -> &str {
        self.socket_path()
    }
}

fn request(intent: &BackendIntent) -> (&'static str, Value) {
    match intent {
        BackendIntent::SplitPane { pane, axis, ratio } => {
            let mut params = json!({
                // `target_pane_id`, not `pane_id`, and the difference is silent: herdr
                // ignores a key it does not know and splits whichever pane it has focused,
                // so the wrong name reads as a split landing in an arbitrary place rather
                // than as a refusal. Every other pane verb takes `pane_id`.
                "target_pane_id": pane.as_str(),
                "direction": direction(*axis),
                // The daemon's cursor follows the new pane, because a person who split
                // something is looking at what they made. Muster's own keyboard is moved
                // separately, by whoever asked for this.
                "focus": true,
            });
            if let Some(ratio) = ratio {
                params["ratio"] = json!(ratio);
            }
            ("pane.split", params)
        }
        BackendIntent::ClosePane { pane } => ("pane.close", json!({ "pane_id": pane.as_str() })),
        BackendIntent::FocusPane { pane } => ("pane.focus", json!({ "pane_id": pane.as_str() })),
        BackendIntent::SetSplitRatio { tab, path, ratio } => (
            "layout.set_split_ratio",
            json!({
                "tab_id": tab.as_str(),
                "path": path.iter().map(|turn| *turn == Branch::Second).collect::<Vec<bool>>(),
                "ratio": ratio,
            }),
        ),
    }
}

/// herdr names a split for where the new pane goes; Muster names it for the arrangement it
/// produces. The same pair `layout.rs` reads in the other direction.
fn direction(axis: SplitAxis) -> &'static str {
    match axis {
        SplitAxis::Columns => "right",
        SplitAxis::Rows => "down",
    }
}
