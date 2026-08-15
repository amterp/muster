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

use muster_core::intent::{BackendChannel, BackendIntent, Branch, Outcome, Refusal};
use muster_core::mirror::backend::{PaneId, SplitAxis, TabId};
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};

impl BackendChannel for HerdrClient {
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, Refusal> {
        let (method, params) = request(intent);
        let result = self.request(method, &params).map_err(|failure| refusal(&failure))?;
        Ok(Outcome { created: created(&result), created_tab: created_tab(&result) })
    }

    fn description(&self) -> &str {
        self.socket_path()
    }
}

/// herdr's refusal, in the two kinds Muster acts on differently.
///
/// The codes it names are the ones that mean "no such thing here", which herdr spells one way
/// per kind of thing (`pane_not_found`, `tab_not_found`, `workspace_not_found`,
/// `layout_not_found`, `split_not_found`). Read by code rather than by message: the messages
/// are prose that has changed between versions and the codes have not, and being wrong here
/// means a window that keeps showing a pane nobody can reach.
///
/// Everything else - unreachable, timed out, a daemon that answered something unparseable -
/// says nothing about whether the pane exists, so it stays a refusal and nothing more.
pub fn refusal(failure: &Failure) -> Refusal {
    let detail = failure.to_string();
    match failure {
        Failure::Daemon { code, .. } if code.ends_with("not_found") => Refusal::NotThere(detail),
        _ => Refusal::Declined(detail),
    }
}

/// One intent as the method and parameters herdr wants for it.
///
/// Public so the corpus can pin it. herdr ignores a parameter it does not recognise, so a
/// misspelled key is not a refusal - it is a request that acts on whatever the daemon had
/// focused, which against a one-pane daemon is indistinguishable from the right answer. That
/// is not something a test with a real daemon in it can catch, so the keys are checked here
/// by name (`corpus/conformance/backend-intent.json`).
pub fn request(intent: &BackendIntent) -> (&'static str, Value) {
    match intent {
        BackendIntent::SplitPane { pane, axis, ratio, cwd } => {
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
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            ("pane.split", params)
        }
        BackendIntent::CreateTab { workspace, cwd } => {
            // `workspace_id`, and nothing else names where this goes. herdr ignores a key it
            // does not know, so a `pane_id` sent hopefully would be dropped in silence and
            // the tab would land in whichever workspace that daemon had focused.
            let mut params = json!({ "workspace_id": workspace.as_str(), "focus": true });
            if let Some(cwd) = cwd {
                params["cwd"] = json!(cwd);
            }
            ("tab.create", params)
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
        BackendIntent::ResizePane { pane, direction, amount } => {
            let mut params = json!({ "pane_id": pane.as_str(), "direction": direction.as_str() });
            if let Some(amount) = amount {
                params["amount"] = json!(amount);
            }
            ("pane.resize", params)
        }
        // `mode` is left to its default, which herdr documents as toggle. Sending it would be
        // Muster restating a default it agrees with, and the day herdr changes that default
        // is the day this should notice rather than silently keep the old one.
        BackendIntent::ZoomPane { pane } => ("pane.zoom", json!({ "pane_id": pane.as_str() })),
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

/// The tab a request made, if it made one.
///
/// `tab.create` answers with the tab under `tab`, and `workspace.create` with the one it
/// started the workspace off with. Read for the same reason the pane is: a tab nothing is
/// showing is a tab nobody asked for twice.
fn created_tab(result: &Value) -> Option<TabId> {
    Some(TabId::new(result.get("tab")?.get("tab_id")?.as_str()?))
}

/// herdr names a split for where the new pane goes; Muster names it for the arrangement it
/// produces. The same pair `layout.rs` reads in the other direction.
fn direction(axis: SplitAxis) -> &'static str {
    match axis {
        SplitAxis::Columns => "right",
        SplitAxis::Rows => "down",
    }
}
