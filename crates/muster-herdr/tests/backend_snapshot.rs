//! Reading a herdr session.snapshot into the mirror's world. Cases and their reasoning
//! live in corpus/conformance/backend-snapshot.json.

use conformance::{Conformance, fields};
use muster_herdr::snapshot::read_snapshot;
use serde_json::{Map, Value, json};

#[test]
fn backend_snapshot_conformance() {
    let corpus = Conformance::load("backend-snapshot.json");

    let ran = corpus.run(|given| {
        let (snapshot, dropped) = read_snapshot(given.get("snapshot").unwrap_or(&Value::Null));

        let mut agent_states = Map::new();
        let mut agents = Map::new();
        for pane in &snapshot.panes {
            agent_states.insert(pane.id.to_string(), json!(pane.agent_state.as_str()));
            if let Some(agent) = &pane.agent {
                agents.insert(pane.id.to_string(), json!(agent));
            }
        }

        Ok(fields([
            (
                "workspaces",
                Some(json!(
                    snapshot.workspaces.iter().map(|w| w.id.to_string()).collect::<Vec<_>>()
                )),
            ),
            (
                "tabs",
                Some(json!(snapshot.tabs.iter().map(|t| t.id.to_string()).collect::<Vec<_>>())),
            ),
            (
                "panes",
                Some(json!(snapshot.panes.iter().map(|p| p.id.to_string()).collect::<Vec<_>>())),
            ),
            ("agentStates", Some(Value::Object(agent_states))),
            ("agents", Some(Value::Object(agents))),
            // Trees on one line, keyed by tab. Rebuilding one is judged in
            // layout-reconstruction.json; what a case here can say is which tabs came back
            // with an arrangement at all, which is what `dropped` below is counting when
            // one does not.
            (
                "layouts",
                Some(Value::Object(
                    snapshot
                        .layouts
                        .iter()
                        .map(|layout| (layout.tab.to_string(), json!(layout.root.to_string())))
                        .collect(),
                )),
            ),
            (
                "focus",
                Some(fields([
                    ("workspace", snapshot.focus.workspace.map(|id| json!(id.to_string()))),
                    ("tab", snapshot.focus.tab.map(|id| json!(id.to_string()))),
                    ("pane", snapshot.focus.pane.map(|id| json!(id.to_string()))),
                ])),
            ),
            // Stated as null rather than omitted when absent, because "herdr sent no
            // counter" is a fact a case should be able to assert rather than express by
            // leaving a key out.
            ("agentStateSeq", Some(snapshot.agent_state_seq.map_or(Value::Null, |s| json!(s)))),
            ("dropped", Some(json!(dropped))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
