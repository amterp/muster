//! Muster's requested changes, in herdr's words.
//!
//! The write half of the adapter, and the mirror image of `events.rs`: everything that knows
//! what a `pane.split` envelope looks like is here, so a second backend is a second file
//! rather than a search for every place a JSON key leaked.
//!
//! Three translations are worth naming. A split's side becomes the direction herdr names it
//! for - a new pane goes on the `second` side of a `right` or a `down` - which is the same
//! translation `layout.rs` does in reverse when it reads a tree back. A path down the tree
//! becomes an array of booleans, herdr's own spelling for the turns. And two of Muster's four
//! sides have no request behind them at all, so this is also where one intent becomes two.

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::intent::{
    BackendChannel, BackendIntent, Branch, Outcome, Refusal, SettledLayout, Side,
};
use muster_core::mirror::backend::{PaneId, TabId};
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};
use crate::layout::{read_exported_layout, read_layout};

impl BackendChannel for HerdrClient {
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, Refusal> {
        let (method, params) = request(intent);
        let result = self.request(method, &params).map_err(|failure| refusal(&failure))?;
        let created = created(&result);
        let settled = match (rearranges(intent), &created) {
            (Some(pane), Some(created)) => self.rearrange(pane, created),
            // A split herdr made and would not name. Worth saying, because the consequence is
            // no longer only a keyboard that stays put: the pane cannot be rearranged either,
            // so a leftward split quietly becomes a rightward one.
            (Some(pane), None) => {
                log::warn(
                    "herdr.split.unnamed",
                    fields! {
                        "pane" => pane.to_string(),
                        "impact" => "the split happened and the new pane is on the opposite \
                                     side from the one that was asked for, because nothing \
                                     can name it to rearrange it.",
                        "next" => "whether pane.split still answers with the pane it made, \
                                   under `pane.pane_id` - `created` in this file reads it.",
                    },
                );
                None
            }
            _ => {
                // A mutation the daemon considered and did not perform. It is a success on the
                // wire, so nothing above notices, and the only symptom is a window that does
                // not change - which is exactly what a bug in the request looks like too.
                if let Some(reason) = declined(&result) {
                    log::warn(
                        "herdr.intent.declined",
                        fields! {
                            "method" => method,
                            "reason" => reason,
                            "impact" => "the window is unchanged and correct - the daemon did \
                                         not do this, and nothing is being rendered that did \
                                         not happen. What was asked for did not take effect.",
                            "next" => "the reason is herdr's own word for why. For a swap those \
                                       are no_neighbor, same_pane, not_found and cross_tab; a \
                                       cross_tab here means the caller should have sent a move.",
                        },
                    );
                }
                // Applied even when declined, because the layout an answer carries is the
                // daemon's current arrangement either way, and a mirror already holding it
                // settles to a no-op.
                settled(&result, None)
            }
        };
        Ok(Outcome {
            created,
            created_tab: created_tab(&result),
            settled,
            renamed: renamed(intent, &result),
        })
    }

    fn description(&self) -> &str {
        self.socket_path()
    }
}

impl HerdrClient {
    /// Puts a pane herdr has just made on the other side of the one it was split from.
    ///
    /// herdr's `SplitDirection` is `right` and `down`, and a split always puts the new pane on
    /// the `second` side, so leftward and upward have no request behind them: they are a split
    /// followed by `pane.swap` of the pair. The whole compound lives here rather than above,
    /// because it is a fact about one daemon's vocabulary and a backend with four directions
    /// would need none of it.
    ///
    /// **A refusal here is not a failure of the intent.** The pane exists - herdr said so
    /// before this was asked - and the honest answer to a swap that will not happen is to keep
    /// it and say where it ended up, never to close a pane the user may already be typing in.
    /// Undoing is the one thing worse than a pane on the wrong side.
    fn rearrange(&self, pane: &PaneId, created: &PaneId) -> Option<SettledLayout> {
        let params = json!({ "source_pane_id": pane.as_str(), "target_pane_id": created.as_str() });
        let answer = match self.request("pane.swap", &params) {
            Ok(answer) => answer,
            Err(failure) => {
                // `invalid_pane_swap` rather than one of the four refusal reasons, for a swap
                // herdr will not even consider (`observations/herdr-0.8.0.md` section 14).
                log::warn(
                    "herdr.swap.refused",
                    fields! {
                        "pane" => pane.to_string(),
                        "created" => created.to_string(),
                        "detail" => failure.to_string(),
                        "impact" => "the new pane is on the opposite side from the one that \
                                     was asked for. Nothing is lying - the daemon and the \
                                     window agree - and the pane is usable.",
                        "next" => "herdr's code says why it would not swap. A four-way \
                                   SplitDirection upstream would remove the second request \
                                   entirely (kan a_28XGcvXEg).",
                    },
                );
                return None;
            }
        };
        // `changed` is a success with a reason beside it rather than an error, and it nests
        // one level down like every other herdr result - read off the top level it is `null`,
        // which is indistinguishable from a swap that did nothing.
        let swap = answer.get("swap")?;
        if swap.get("changed").and_then(Value::as_bool) != Some(true) {
            log::warn(
                "herdr.swap.declined",
                fields! {
                    "pane" => pane.to_string(),
                    "created" => created.to_string(),
                    "reason" => swap.get("reason").and_then(Value::as_str).unwrap_or("(none given)"),
                    "impact" => "the new pane is on the opposite side from the one that was \
                                 asked for, and is usable.",
                    "next" => "herdr names four reasons a swap does not happen: no_neighbor, \
                               same_pane, not_found, cross_tab.",
                },
            );
            return None;
        }
        settled(swap, Some((pane, created)))
    }
}

/// Why the daemon did nothing, for a result that says it did nothing.
///
/// `changed: false` is a success with a reason beside it rather than an error, and it nests one
/// level down like every other herdr result - read off the top level it is null, which is
/// indistinguishable from a mutation that worked. A verb that never reports one answers `None`
/// here and is left alone.
fn declined(result: &Value) -> Option<&str> {
    let changed = nested(result, "changed")?.as_bool()?;
    (!changed).then(|| nested(result, "reason").and_then(Value::as_str).unwrap_or("(none given)"))
}

/// The pane a split has to be rearranged against, for an intent herdr cannot do in one request.
fn rearranges(intent: &BackendIntent) -> Option<&PaneId> {
    match intent {
        BackendIntent::SplitPane { pane, side, .. } if swaps(*side) => Some(pane),
        _ => None,
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
        BackendIntent::SplitPane { pane, side, ratio, cwd } => {
            let mut params = json!({
                // `target_pane_id`, not `pane_id`, and the difference is silent: herdr
                // ignores a key it does not know and splits whichever pane it has focused,
                // so the wrong name reads as a split landing in an arbitrary place rather
                // than as a refusal. Every other pane verb takes `pane_id`.
                "target_pane_id": pane.as_str(),
                "direction": direction(*side),
                // The daemon's cursor follows the new pane, because a person who split
                // something is looking at what they made. Muster's own keyboard is moved
                // separately, by whoever asked for this.
                "focus": true,
            });
            if let Some(ratio) = ratio {
                // herdr's `ratio` is the *first* child's share, and Muster's is the share the
                // pane being split keeps. On a leftward or upward split that pane ends up
                // second, so the two count from opposite ends and the number has to be turned
                // round. Invisible to a keybinding, which sends no ratio at all and takes the
                // daemon's symmetric default; visible to anything that places a divider.
                params["ratio"] = json!(if swaps(*side) { 1.0 - ratio } else { *ratio });
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
        // `source_pane_id` and `target_pane_id`, the same pair the leftward-split rearrange
        // sends. Note that herdr moves its own cursor to the *source* pane whatever was
        // focused before (`observations/herdr-0.8.0.md` section 14); Muster's keyboard is its
        // own and is not moved by this.
        BackendIntent::SwapPanes { pane, with } => (
            "pane.swap",
            json!({ "source_pane_id": pane.as_str(), "target_pane_id": with.as_str() }),
        ),
        // `destination` is a tagged object rather than a bare id: herdr's `pane.move` can also
        // make a tab or a workspace to move into, and the tag is how it tells those apart.
        // Muster only ever names an existing tab, because a drop landed on a row in one.
        //
        // "after" becomes a rightward split of the pane it lands behind. herdr puts a new pane
        // on the `second` side and reads a split first-then-second, so `right` is exactly one
        // place later in the order the agent list shows - the ordering the intent asked for,
        // spelled in the only geometry herdr's `split` accepts (it takes right and down alone).
        //
        // `focus` is left false. Moving a pane is arranging the window rather than going
        // somewhere, and a drag that also took the keyboard would interrupt whatever is being
        // typed into the pane that had it.
        BackendIntent::MovePane { pane, tab, after } => (
            "pane.move",
            json!({
                "pane_id": pane.as_str(),
                "destination": {
                    "type": "tab",
                    "tab_id": tab.as_str(),
                    "target_pane_id": after.as_str(),
                    "split": "right",
                },
                "focus": false,
            }),
        ),
        BackendIntent::SetSplitRatio { tab, path, ratio } => (
            "layout.set_split_ratio",
            json!({
                "tab_id": tab.as_str(),
                "path": path.iter().map(|turn| *turn == Branch::Second).collect::<Vec<bool>>(),
                "ratio": ratio,
            }),
        ),
        // Null rather than an absent key, because herdr's `label` is nullable here and null is
        // its spelling for taking the name away. Sending nothing would leave the name alone,
        // which is a different request.
        BackendIntent::RenamePane { pane, name } => (
            "pane.rename",
            json!({
                "pane_id": pane.as_str(),
                "label": name.as_ref().map_or(Value::Null, |name| json!(name)),
            }),
        ),
        // An empty string where the pane above sends null, because herdr's `tab.rename` takes a
        // required string and refuses a null. It does not restore the tab's number either - the
        // tab is left holding an empty name that survives a daemon restart
        // (`observations/herdr-0.8.0.md` section 16). Muster's own caption reads that as
        // unnamed and draws the place, so the window is right and herdr's own interface is not.
        BackendIntent::RenameTab { tab, name } => (
            "tab.rename",
            json!({ "tab_id": tab.as_str(), "label": name.clone().unwrap_or_default() }),
        ),
    }
}

/// What a pane is called now, for the request that renamed one.
///
/// Read from the answer because there is nowhere else to read it: herdr emits no event for a
/// pane rename and has no topic for one, so this reply is the only thing that ever says the
/// name changed (`observations/herdr-0.8.0.md` section 16).
///
/// Keyed off the intent rather than off the shape of the answer, because `pane.rename` replies
/// with the same pane payload several other methods do - taking it from any of them would let
/// a focus or a resize restate a name it knows nothing about.
///
/// The pane comes from the intent and not from the reply. The reply names it too and agrees,
/// but the intent is what this window asked about, and a mirror updated from the answer's own
/// idea of which pane it was is a mirror that cannot notice the two disagreeing.
fn renamed(intent: &BackendIntent, result: &Value) -> Option<(PaneId, Option<String>)> {
    let BackendIntent::RenamePane { pane, .. } = intent else { return None };
    let name = result
        .get("pane")
        .and_then(|pane| pane.get("label"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Some((pane.clone(), name))
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

/// The axis herdr splits along to produce a side, named the way herdr names it.
///
/// Two of Muster's four sides map onto the same request as their opposite, because herdr has
/// no other: a leftward split is a rightward one that has not been rearranged yet. The pair
/// `layout.rs` reads back in the other direction is this one with the sides dropped.
fn direction(side: Side) -> &'static str {
    match side {
        Side::Left | Side::Right => "right",
        Side::Up | Side::Down => "down",
    }
}

/// Whether herdr puts a new pane on this side only after being asked a second time.
fn swaps(side: Side) -> bool {
    matches!(side, Side::Left | Side::Up)
}

/// The arrangement a herdr result states, when it states one.
///
/// Reading a mutation's own answer rather than waiting to be told is what keeps the window on
/// the arrangement that exists: herdr broadcasts the same tree about a hundred milliseconds
/// later, and a client that only listens renders the arrangement being moved away from for six
/// frames and then jumps (`observations/herdr-0.8.0.md` section 14).
///
/// Two shapes, because herdr states one in two ways. `pane.swap` and `pane.resize` answer with
/// the flat rectangles `layout_updated` carries; `layout.set_split_ratio` answers with the
/// exported tree. Tried in that order and with no branch on the verb - which shape a result
/// carries is the result's own business, and naming each one here would be a table to keep in
/// step with a daemon that ships weekly.
///
/// A result that carries neither is `None`, which is the same answer as a result that states
/// no arrangement at all - and the right one, because the caller then waits for the broadcast,
/// which is what every arrangement change did before any of this.
///
/// `swapped` names the pair a compound intent exchanged, and is what lets the arrangement
/// herdr published between the two halves be reconstructed: its swap exchanges the ids sitting
/// in two places and leaves the places alone, so the tree it was is the tree it became with
/// those two ids put back.
fn settled(result: &Value, swapped: Option<(&PaneId, &PaneId)>) -> Option<SettledLayout> {
    let stated = nested(result, "layout")?;
    let layout = read_layout(stated).or_else(|| read_exported_layout(stated))?;
    let stale = swapped.map(|(one, other)| layout.with_panes_exchanged(one, other));
    Some(SettledLayout { layout, stale })
}

/// A field of a herdr result, wherever the result nests it.
///
/// Every result puts its payload under a key beside its `type` rather than at the top level
/// (`observations/herdr-0.8.0.md` section 6), and which key differs per verb. Looking one
/// level down rather than naming each one means a verb that starts answering with a layout
/// needs no change here.
fn nested<'a>(result: &'a Value, field: &str) -> Option<&'a Value> {
    if let Some(found) = result.get(field) {
        return Some(found);
    }
    result
        .as_object()?
        .iter()
        .find_map(|(key, value)| (key != "type").then(|| value.get(field)).flatten())
}
