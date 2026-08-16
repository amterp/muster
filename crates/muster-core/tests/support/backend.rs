//! A daemon's world, as a case spells it.
//!
//! One reader, because more than one corpus describes a session and two readers would let
//! the same JSON mean two things - which is the failure the whole conformance arrangement
//! exists to prevent (`crates/conformance/src/lib.rs`).

use std::fmt::Write as _;

use muster_core::AgentState;
use muster_core::composition::{Daemon, Endpoint};
use muster_core::mirror::backend::{
    Focus, Layout, LayoutNode, Pane, PaneId, Snapshot, SplitAxis, Tab, TabId, Workspace,
    WorkspaceId,
};
use serde_json::Value;

pub(crate) fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// One attached daemon, as a line.
///
/// Here rather than in either driver because two corpora expect these strings - a config
/// file produces daemons and a composition holds them - and two renderings would let the
/// same line mean two things in two files.
pub(crate) fn describe_daemon(daemon: &Daemon) -> String {
    let mut line = daemon.id.to_string();
    match &daemon.endpoint {
        Endpoint::Local { socket_path } => {
            line.push_str(" local");
            if let Some(path) = socket_path {
                let _ = write!(line, "={path}");
            }
        }
        Endpoint::Ssh { host, options, socket_path } => {
            let _ = write!(line, " ssh={host}");
            if let Some(path) = socket_path {
                let _ = write!(line, " socket={path}");
            }
            if !options.is_empty() {
                let _ = write!(line, " options={}", options.join(" "));
            }
        }
    }
    line
}

pub(crate) fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn read_snapshot(given: &Value) -> Snapshot {
    let focus = given.get("focus").cloned().unwrap_or(Value::Null);
    Snapshot {
        workspaces: collect(given, "workspaces", |w| Workspace {
            id: WorkspaceId::new(text(w, "id")),
            label: text(w, "label"),
        }),
        tabs: collect(given, "tabs", |t| Tab {
            id: TabId::new(text(t, "id")),
            workspace: WorkspaceId::new(text(t, "workspace")),
            label: text(t, "label"),
        }),
        panes: collect(given, "panes", read_pane),
        layouts: collect(given, "layouts", read_layout),
        focus: Focus {
            workspace: optional(&focus, "workspace").map(WorkspaceId::new),
            tab: optional(&focus, "tab").map(TabId::new),
            pane: optional(&focus, "pane").map(PaneId::new),
        },
        agent_state_seq: given.get("agentStateSeq").and_then(Value::as_u64),
    }
}

fn collect<T>(given: &Value, key: &str, read: impl Fn(&Value) -> T) -> Vec<T> {
    given.get(key).and_then(Value::as_array).into_iter().flatten().map(read).collect()
}

pub(crate) fn read_pane(given: &Value) -> Pane {
    Pane {
        id: PaneId::new(text(given, "id")),
        tab: TabId::new(text(given, "tab")),
        workspace: WorkspaceId::new(text(given, "workspace")),
        agent_state: AgentState::from_backend(&text(given, "agentState")),
        agent: optional(given, "agent"),
        cwd: text(given, "cwd"),
        name: optional(given, "name"),
        title: optional(given, "title"),
    }
}

pub(crate) fn read_layout(given: &Value) -> Layout {
    Layout {
        tab: TabId::new(text(given, "tab")),
        root: read_node(given.get("root").unwrap_or(&Value::Null)),
        focused: optional(given, "focused").map(PaneId::new),
        zoomed: optional(given, "zoomed").map(PaneId::new),
    }
}

/// A tree written the way a case reads best: a string is a pane, an object is a split.
///
/// The alternative - a `type` discriminant on every node - triples the size of a case for
/// no information, and these cases are meant to be read.
pub(crate) fn read_node(given: &Value) -> LayoutNode {
    if let Some(pane) = given.as_str() {
        return LayoutNode::Pane(PaneId::new(pane));
    }
    let axis = match text(given, "axis").as_str() {
        "columns" => SplitAxis::Columns,
        "rows" => SplitAxis::Rows,
        other => panic!("corpus case names a split axis the driver does not know: {other:?}"),
    };
    LayoutNode::Split {
        axis,
        ratio: ratio(given),
        first: Box::new(read_node(given.get("first").unwrap_or(&Value::Null))),
        second: Box::new(read_node(given.get("second").unwrap_or(&Value::Null))),
    }
}

/// JSON has one number type and a ratio is an `f32`, so narrowing is what reading one is.
///
/// Shared, because two corpora spell a ratio - a pane's split and a region's boundary - and
/// two readers would let the same number mean two things.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn ratio(given: &Value) -> f32 {
    given.get("ratio").and_then(Value::as_f64).unwrap_or(0.5) as f32
}
