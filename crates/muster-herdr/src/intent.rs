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

use muster_core::intent::{BackendChannel, BackendIntent, Branch, Outcome};
use muster_core::mirror::backend::{PaneId, SplitAxis};
use serde_json::{Value, json};

use crate::client::HerdrClient;

impl BackendChannel for HerdrClient {
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, String> {
        let (method, params) = request(intent);
        let result = self.request(method, &params).map_err(|failure| failure.to_string())?;
        Ok(Outcome { created: created(&result) })
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
        BackendIntent::CreateWorkspace { cwd } => {
            let mut params = json!({
                // The daemon's cursor follows it, for the same reason a split's does: this is
                // asked for by somebody who wants to be looking at what it makes.
                "focus": true,
                // herdr labels an unlabelled workspace itself, and a name Muster invented
                // would be a second naming scheme for the same thing.
                "label": Value::Null,
            });
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            ("workspace.create", params)
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

/// The pane a request made, if it made one.
///
/// `pane.split` answers with the whole new pane nested under `pane`, and `workspace.create`
/// with the one it started the workspace off with under `root_pane` - the way every herdr
/// result nests under a key beside its `type`. Only the id is read: everything else about
/// them arrives on the event stream a moment later, and reading it here would be a second
/// source for facts the mirror already owns.
///
/// A shape that does not match is `None` rather than a refusal. The split happened - the
/// daemon said so - and the only cost of not finding the id is a keyboard that stays where it
/// was, which is worth less than turning a successful split into an error.
fn created(result: &Value) -> Option<PaneId> {
    let pane = result.get("pane").or_else(|| result.get("root_pane"))?;
    Some(PaneId::new(pane.get("pane_id")?.as_str()?))
}

/// herdr names a split for where the new pane goes; Muster names it for the arrangement it
/// produces. The same pair `layout.rs` reads in the other direction.
fn direction(axis: SplitAxis) -> &'static str {
    match axis {
        SplitAxis::Columns => "right",
        SplitAxis::Rows => "down",
    }
}
