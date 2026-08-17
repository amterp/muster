//! Turns a herdr `session.snapshot` into the mirror's picture of a session.
//!
//! The other half of the translation in `events.rs`, and the one that runs first: a
//! subscription describes changes to a world, and this is where the world comes from. Also
//! the only route by which the mirror learns it missed something, since herdr's transition
//! counter reaches a client here and nowhere else
//! (`observations/herdr-0.8.0.md` section 10).

use std::collections::BTreeMap;
use std::time::Duration;

use muster_core::AgentState;
use muster_core::mirror::backend::{Focus, Pane, PaneId, Snapshot, Tab, Workspace, WorkspaceId};
use muster_core::names::Names;
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};
use crate::layout::{PaneCells, read_layout, unattached_sizes};

/// Asks a daemon for its whole session, once.
///
/// The subscription makes the same call on every connect and reconnect, so this is the
/// second caller rather than a second mechanism. It exists because attaching needs an
/// answer before it can say anything: a window that does not yet know which tab its pane is
/// in has nowhere to put it, and the subscription's own bootstrap arrives on another thread
/// some milliseconds later.
///
/// Asking twice costs nothing. Bootstrap replaces rather than merges, and replacing a
/// picture with the same picture reports no changes at all.
pub fn fetch_snapshot(socket_path: &str, names: &Names) -> Result<(Snapshot, usize), Failure> {
    fetch_snapshot_within(socket_path, names, HerdrClient::DEFAULT_TIMEOUT)
}

/// The same, for a caller that can say how long the answer is worth waiting for.
///
/// The default above is the client's, which is short because that client sits on the input
/// path and a wedged daemon must not take the keyboard with it. A caller for whom nothing
/// renders until this answers is not on the input path and should not be held to a keystroke's
/// budget - the subscription's bootstrap is the one that is not, and it says why.
pub fn fetch_snapshot_within(
    socket_path: &str,
    names: &Names,
    allowance: Duration,
) -> Result<(Snapshot, usize), Failure> {
    let client = HerdrClient::new(socket_path.to_string());
    let result = client.request_within("session.snapshot", &json!({}), allowance)?;
    Ok(read_snapshot(result.get("snapshot").unwrap_or(&Value::Null), names))
}

/// Asks a daemon how big each of its panes should be with nothing driving it.
///
/// The same call as [`fetch_snapshot`] and a different reader, because the answer is one the
/// mirror deliberately does not keep: the rectangles are cells in a terminal area herdr keeps
/// for itself, and the moment one reaches the core it looks like geometry a window could use
/// (`layout.rs`). What comes back here is a size to hand a pane back at, and nothing renders
/// from it.
///
/// One call per daemon rather than one per pane, because the caller is a window on its way out
/// and a round trip per pane is fifteen of them between the chord and the process ending.
pub fn fetch_unattached_sizes(
    socket_path: &str,
    names: &Names,
) -> Result<BTreeMap<PaneId, PaneCells>, Failure> {
    let client = HerdrClient::new(socket_path.to_string());
    let result = client.request("session.snapshot", &json!({}))?;
    Ok(unattached_sizes(result.get("snapshot").unwrap_or(&Value::Null), names))
}

/// How many rows one pane's terminal has right now, by the daemon's own account.
///
/// The only dimension herdr reports. A pane's payload carries `scroll.viewport_rows` and no
/// columns at all (`corpus/herdr-0.8.0/api-schema.json`), which makes this half an oracle - and
/// half is enough for the one caller: a window on its way out, checking that the resize it sent
/// down a pane's control stream actually arrived before the bridge relaying it is killed.
///
/// `None` for a daemon that will not answer or a pane it no longer holds. Both mean the same
/// thing to that caller - stop waiting - and neither is worth a record of its own while the
/// window is closing.
pub fn pane_rows(socket_path: &str, names: &Names, pane: &PaneId) -> Option<u16> {
    let client = HerdrClient::new(socket_path.to_string());
    let backend = names.backend_pane(pane).ok()?;
    let result = client.request("pane.get", &json!({ "pane_id": backend.as_str() })).ok()?;
    let rows = result.get("pane")?.get("scroll")?.get("viewport_rows")?.as_u64()?;
    u16::try_from(rows).ok()
}

/// Reads the `snapshot` object out of a `session.snapshot` result.
///
/// Absent lists read as empty rather than as a refusal. A daemon with no workspaces is a
/// daemon someone just started, and it is a session Muster should render as empty rather
/// than as broken - the difference between the two is `Health`, which the caller owns.
///
/// Entries that will not read are skipped rather than failing the whole snapshot, for the
/// same reason a bad line does not kill a stream: one unreadable pane should cost that
/// pane, not the session. `dropped` says how many, so a caller can say so out loud rather
/// than rendering a quietly smaller world.
pub fn read_snapshot(snapshot: &Value, names: &Names) -> (Snapshot, usize) {
    let mut dropped = 0;

    let workspaces = collect(snapshot, "workspaces", &mut dropped, |value| {
        Some(Workspace {
            id: WorkspaceId::new(id(value, "workspace_id")?),
            label: text(value, "label").to_string(),
        })
    });
    let tabs = collect(snapshot, "tabs", &mut dropped, |value| {
        Some(Tab {
            id: names.tab(&id(value, "tab_id")?),
            workspace: WorkspaceId::new(id(value, "workspace_id")?),
            label: text(value, "label").to_string(),
        })
    });
    // From `panes`, not from `agents`. The two overlap - `agents` is the subset running
    // one, carrying the same fields plus the counter - and reading the world from the
    // subset would lose every pane that has no agent, which is most of them.
    let panes = collect(snapshot, "panes", &mut dropped, |value| {
        Some(Pane {
            id: names.pane(&id(value, "pane_id")?),
            tab: names.tab(&id(value, "tab_id")?),
            workspace: WorkspaceId::new(id(value, "workspace_id")?),
            agent_state: AgentState::from_backend(text(value, "agent_status")),
            agent: value.get("agent").and_then(Value::as_str).map(str::to_string),
            cwd: text(value, "cwd").to_string(),
            name: optional(value, "label"),
            revision: value.get("revision").and_then(Value::as_u64).unwrap_or_default(),
            title: optional(value, "terminal_title_stripped"),
        })
    });

    // Same object as a `layout_updated` carries, which is why one reader serves both. A
    // tab whose arrangement will not read is counted as dropped and left absent: the
    // mirror renders a tab it has no tree for as the tree it had, which is a better wrong
    // answer than an empty tab.
    let layouts = collect(snapshot, "layouts", &mut dropped, |layout| read_layout(layout, names));

    // The highest stamp, not the count of agents: it is one session-wide counter, so the
    // highest value is what the session has run in total, and a pane that has never had an
    // agent contributes nothing to it.
    let agent_state_seq = snapshot
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|agent| agent.get("state_change_seq").and_then(Value::as_u64))
        .max();

    let snapshot = Snapshot {
        workspaces,
        tabs,
        panes,
        layouts,
        focus: Focus {
            workspace: id(snapshot, "focused_workspace_id").map(WorkspaceId::new),
            tab: id(snapshot, "focused_tab_id").map(|tab| names.tab(&tab)),
            pane: id(snapshot, "focused_pane_id").map(|pane| names.pane(&pane)),
        },
        agent_state_seq,
    };
    (snapshot, dropped)
}

fn collect<T>(
    snapshot: &Value,
    key: &str,
    dropped: &mut usize,
    read: impl Fn(&Value) -> Option<T>,
) -> Vec<T> {
    let mut out = Vec::new();
    for value in snapshot.get(key).and_then(Value::as_array).into_iter().flatten() {
        match read(value) {
            Some(item) => out.push(item),
            None => *dropped += 1,
        }
    }
    out
}

/// A required identifier: present, a string, and not empty. Empty is refused for the same
/// reason as on the event path - it is a lookup key that finds nothing while looking like
/// an entity nobody created.
fn id(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// A field that may be absent, null, or empty, where all three mean "nothing to show".
/// Same reasoning as on the event path: `""` and nothing differ by a blank line on screen.
fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string)
}
