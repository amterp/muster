//! Turns a herdr `session.snapshot` into the mirror's picture of a session.
//!
//! The other half of the translation in `events.rs`, and the one that runs first: a
//! subscription describes changes to a world, and this is where the world comes from. Also
//! the only route by which the mirror learns it missed something, since herdr's transition
//! counter reaches a client here and nowhere else
//! (`observations/herdr-0.8.0.md` section 10).

use muster_core::AgentState;
use muster_core::mirror::backend::{
    Focus, Pane, PaneId, Snapshot, Tab, TabId, Workspace, WorkspaceId,
};
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};
use crate::layout::read_layout;

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
pub fn fetch_snapshot(socket_path: &str) -> Result<(Snapshot, usize), Failure> {
    let client = HerdrClient::new(socket_path.to_string());
    let result = client.request("session.snapshot", &json!({}))?;
    Ok(read_snapshot(result.get("snapshot").unwrap_or(&Value::Null)))
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
pub fn read_snapshot(snapshot: &Value) -> (Snapshot, usize) {
    let mut dropped = 0;

    let workspaces = collect(snapshot, "workspaces", &mut dropped, |value| {
        Some(Workspace {
            id: WorkspaceId::new(id(value, "workspace_id")?),
            label: text(value, "label").to_string(),
        })
    });
    let tabs = collect(snapshot, "tabs", &mut dropped, |value| {
        Some(Tab {
            id: TabId::new(id(value, "tab_id")?),
            workspace: WorkspaceId::new(id(value, "workspace_id")?),
            label: text(value, "label").to_string(),
        })
    });
    // From `panes`, not from `agents`. The two overlap - `agents` is the subset running
    // one, carrying the same fields plus the counter - and reading the world from the
    // subset would lose every pane that has no agent, which is most of them.
    let panes = collect(snapshot, "panes", &mut dropped, |value| {
        Some(Pane {
            id: PaneId::new(id(value, "pane_id")?),
            tab: TabId::new(id(value, "tab_id")?),
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
    let layouts = collect(snapshot, "layouts", &mut dropped, read_layout);

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
            tab: id(snapshot, "focused_tab_id").map(TabId::new),
            pane: id(snapshot, "focused_pane_id").map(PaneId::new),
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
