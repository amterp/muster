//! The translation from herdr's wire to Muster's vocabulary. Cases and their reasoning
//! live in corpus/conformance/backend-events.json.

use std::fmt::Write as _;

use conformance::{Conformance, fields};
use muster_core::mirror::BackendEvent;
use muster_herdr::EventDecoder;
use serde_json::{Value, json};

#[test]
fn backend_events_conformance() {
    let corpus = Conformance::load("backend-events.json");

    let ran = corpus.run(|given| {
        let mut decoder = EventDecoder::new();
        let mut events = Vec::new();
        let mut unknown = Vec::new();

        // Chunk by chunk rather than joined, because how the stream was cut is the point
        // of several cases and joining them first would quietly test something easier.
        for chunk in given.get("chunks").and_then(Value::as_array).into_iter().flatten() {
            let bytes = chunk.as_str().unwrap_or_default().as_bytes();
            events.extend(decoder.consume(bytes).iter().map(describe));
            unknown.extend(decoder.take_unknown_kinds());
        }

        Ok(fields([("events", Some(json!(events))), ("unknownKinds", Some(json!(unknown)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// Events render as readable lines rather than as nested objects.
///
/// The corpus is read by whoever is deciding whether an expectation is right, and
/// `paneUpserted:w1:p1 tab=w1:t1 workspace=w1 state=idle cwd=/tmp` says it where a nested
/// object makes them assemble it (docs/testing.md: bytes render readably).
fn describe(event: &BackendEvent) -> String {
    match event {
        BackendEvent::WorkspaceUpserted(workspace) => {
            format!("workspaceUpserted:{} label={}", workspace.id, workspace.label)
        }
        BackendEvent::WorkspaceRemoved(id) => format!("workspaceRemoved:{id}"),
        BackendEvent::TabUpserted(tab) => {
            format!("tabUpserted:{} workspace={} label={}", tab.id, tab.workspace, tab.label)
        }
        BackendEvent::TabRemoved(id) => format!("tabRemoved:{id}"),
        BackendEvent::PaneUpserted(pane) => format!(
            "paneUpserted:{} tab={} workspace={} state={} cwd={}",
            pane.id,
            pane.tab,
            pane.workspace,
            pane.agent_state.as_str(),
            pane.cwd
        ),
        BackendEvent::PaneRemoved(id) => format!("paneRemoved:{id}"),
        BackendEvent::AgentStateChanged { pane, state } => {
            format!("agentStateChanged:{pane} state={}", state.as_str())
        }
        BackendEvent::AgentDetected { pane, agent } => {
            format!("agentDetected:{pane} agent={agent}")
        }
        BackendEvent::FocusMoved { workspace, tab, pane } => {
            let mut out = String::from("focusMoved");
            for (name, id) in [
                ("workspace", workspace.as_ref().map(ToString::to_string)),
                ("tab", tab.as_ref().map(ToString::to_string)),
                ("pane", pane.as_ref().map(ToString::to_string)),
            ] {
                // An unset cursor is absent rather than empty: the mirror reads absence as
                // "says nothing about this one", and the rendering should not blur that.
                if let Some(id) = id {
                    let _ = write!(out, " {name}={id}");
                }
            }
            out
        }
    }
}
