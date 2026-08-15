//! The mirror is the core's picture of daemon truth, and everything above it renders from
//! that picture. Cases and their reasoning live in corpus/conformance/mirror.json.

mod support;

use conformance::{Conformance, fields};
use muster_core::AgentState;
use muster_core::mirror::backend::{Focus, PaneId, Tab, TabId, Workspace, WorkspaceId};
use muster_core::mirror::{BackendEvent, Change, Mirror};
use serde_json::{Value, json};
use support::backend::{optional, read_layout, read_pane, read_snapshot, text};

#[test]
fn mirror_conformance() {
    let corpus = Conformance::load("mirror.json");

    let ran = corpus.run(|given| {
        let mut mirror = Mirror::new();
        let mut changes = Vec::new();

        // Snapshot, then stream, then snapshot again - the order a real connection takes,
        // so that a case about a reconnect is a case about what the mirror had been told
        // before the drop rather than about a bare pair of snapshots.
        if let Some(snapshot) = given.get("snapshot") {
            mirror.bootstrap(read_snapshot(snapshot));
        }
        for event in given.get("events").and_then(Value::as_array).into_iter().flatten() {
            changes.extend(mirror.apply(read_event(event)));
        }
        if let Some(resnapshot) = given.get("resnapshot") {
            changes.extend(mirror.bootstrap(read_snapshot(resnapshot)));
        }

        // Every field, every case. The corpus compares whole objects, and for a state
        // machine that is the point: a case asserting only what it is about would miss a
        // change that also clobbered focus, which is exactly the bug this file caught.
        Ok(fields([
            ("panes", Some(json!(ids(mirror.panes().map(|p| p.id.as_str()))))),
            ("tabs", Some(json!(ids(mirror.tabs().map(|t| t.id.as_str()))))),
            ("workspaces", Some(json!(ids(mirror.workspaces().map(|w| w.id.as_str()))))),
            ("agentStates", Some(agent_states(&mirror))),
            ("layouts", Some(layouts(&mirror))),
            ("focus", Some(focus(mirror.focus()))),
            ("health", Some(json!(mirror.health().as_str()))),
            ("changes", Some(json!(changes.iter().map(describe).collect::<Vec<_>>()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn ids<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    values.map(str::to_string).collect()
}

/// Only the cursors that point at something.
///
/// An unset cursor is absent rather than null, so a case about panes does not carry three
/// nulls describing focus it is not asserting anything about.
fn focus(focus: &Focus) -> Value {
    fields([
        ("workspace", focus.workspace.as_ref().map(|id| json!(id.as_str()))),
        ("tab", focus.tab.as_ref().map(|id| json!(id.as_str()))),
        ("pane", focus.pane.as_ref().map(|id| json!(id.as_str()))),
    ])
}

fn agent_states(mirror: &Mirror) -> Value {
    let mut map = serde_json::Map::new();
    for pane in mirror.panes() {
        map.insert(pane.id.to_string(), json!(pane.agent_state.as_str()));
    }
    Value::Object(map)
}

/// Each tab's tree on one line, keyed by tab.
fn layouts(mirror: &Mirror) -> Value {
    let mut map = serde_json::Map::new();
    for layout in mirror.layouts() {
        let mut described = layout.root.to_string();
        if let Some(zoomed) = &layout.zoomed {
            described = format!("{described} zoomed={zoomed}");
        }
        map.insert(layout.tab.to_string(), json!(described));
    }
    Value::Object(map)
}

/// Changes render as readable strings rather than as nested objects.
///
/// A corpus is read by people deciding whether an expectation is right, and
/// `paneRemoved:w1:p4:cascaded` says what happened where a three-field object makes the
/// reader assemble it (docs/testing.md: bytes render readably).
fn describe(change: &Change) -> String {
    match change {
        Change::PaneAdded(pane) => format!("paneAdded:{pane}"),
        Change::PaneRemoved { pane, cascaded } => {
            let how = if *cascaded { "cascaded" } else { "announced" };
            format!("paneRemoved:{pane}:{how}")
        }
        Change::AgentStateChanged { pane, from, to } => {
            format!("agentStateChanged:{pane}:{}->{}", from.as_str(), to.as_str())
        }
        Change::PaneRelabelled(pane) => format!("paneRelabelled:{pane}"),
        Change::TabAdded(tab) => format!("tabAdded:{tab}"),
        Change::TabRemoved(tab) => format!("tabRemoved:{tab}"),
        Change::LayoutChanged(tab) => format!("layoutChanged:{tab}"),
        Change::WorkspaceAdded(workspace) => format!("workspaceAdded:{workspace}"),
        Change::WorkspaceRemoved(workspace) => format!("workspaceRemoved:{workspace}"),
        Change::FocusChanged => "focusChanged".to_string(),
        Change::AgentTransitionsMissed { expected, saw } => {
            format!("agentTransitionsMissed:{expected}..{saw}")
        }
    }
}

fn read_event(given: &Value) -> BackendEvent {
    match text(given, "kind").as_str() {
        "workspaceUpserted" => BackendEvent::WorkspaceUpserted(Workspace {
            id: WorkspaceId::new(text(given, "id")),
            label: text(given, "label"),
        }),
        "workspaceRemoved" => BackendEvent::WorkspaceRemoved(WorkspaceId::new(text(given, "id"))),
        "tabUpserted" => BackendEvent::TabUpserted(Tab {
            id: TabId::new(text(given, "id")),
            workspace: WorkspaceId::new(text(given, "workspace")),
            label: text(given, "label"),
        }),
        "tabRemoved" => BackendEvent::TabRemoved(TabId::new(text(given, "id"))),
        "paneUpserted" => BackendEvent::PaneUpserted(read_pane(given)),
        "paneRemoved" => BackendEvent::PaneRemoved(PaneId::new(text(given, "id"))),
        "layoutUpserted" => BackendEvent::LayoutUpserted(read_layout(given)),
        "agentStateChanged" => BackendEvent::AgentStateChanged {
            pane: PaneId::new(text(given, "pane")),
            state: AgentState::from_backend(&text(given, "state")),
        },
        "agentDetected" => BackendEvent::AgentDetected {
            pane: PaneId::new(text(given, "pane")),
            agent: text(given, "agent"),
        },
        "focusMoved" => BackendEvent::FocusMoved {
            workspace: optional(given, "workspace").map(WorkspaceId::new),
            tab: optional(given, "tab").map(TabId::new),
            pane: optional(given, "pane").map(PaneId::new),
        },
        // Loudly, because a case naming an event this driver cannot build would otherwise
        // pass by exercising nothing at all.
        other => panic!("corpus case names an event kind the driver does not know: {other:?}"),
    }
}
