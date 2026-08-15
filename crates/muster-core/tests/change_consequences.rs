//! What a change obliges its readers to do. Cases and reasoning live in
//! corpus/conformance/change-consequences.json.
//!
//! These used to be `match` arms inside the seam, where the only way to reach them was a
//! process-global session that permits one test. Being unreachable is why one of them was
//! wrong for as long as it was.

use conformance::{CaseError, Conformance, fields};
use muster_core::AgentState;
use muster_core::mirror::Change;
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use serde_json::{Value, json};

#[test]
fn change_consequences_conformance() {
    let corpus = Conformance::load("change-consequences.json");

    let ran = corpus.run(|given| {
        let change = read_change(given)?;
        Ok(fields([
            ("movesStructure", Some(json!(change.moves_structure()))),
            (
                "announcesAgentState",
                Some(match change.announces_agent_state() {
                    Some(pane) => json!(pane.to_string()),
                    None => Value::Null,
                }),
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn every_change_is_in_the_corpus() {
    // A variant added to `Change` and not here is a change whose consequences nobody
    // decided - and the default a reader falls into is `movesStructure`, which is the
    // expensive answer rather than the wrong one. Silence is the failure mode either way.
    let corpus = Conformance::load("change-consequences.json");
    let covered: Vec<&str> = corpus
        .cases
        .iter()
        .filter_map(|case| case.given.get("change").and_then(Value::as_str))
        .collect();

    for kind in [
        "paneAdded",
        "paneRemoved",
        "agentStateChanged",
        "tabAdded",
        "tabRemoved",
        "layoutChanged",
        "workspaceAdded",
        "workspaceRemoved",
        "focusChanged",
        "agentTransitionsMissed",
    ] {
        assert!(
            covered.contains(&kind),
            "no corpus case gives a `{kind}`, so nothing pins what it obliges a reader to do"
        );
    }
}

fn read_change(given: &Value) -> Result<Change, CaseError> {
    let text = |key: &str| -> Result<String, CaseError> {
        given
            .get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))
    };
    let number = |key: &str| -> Result<u64, CaseError> {
        given
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))
    };

    Ok(match text("change")?.as_str() {
        "paneAdded" => Change::PaneAdded(PaneId::new(text("pane")?)),
        "paneRemoved" => Change::PaneRemoved {
            pane: PaneId::new(text("pane")?),
            cascaded: given.get("cascaded").and_then(Value::as_bool).unwrap_or(false),
        },
        "agentStateChanged" => Change::AgentStateChanged {
            pane: PaneId::new(text("pane")?),
            from: AgentState::from_backend(&text("from")?),
            to: AgentState::from_backend(&text("to")?),
        },
        "tabAdded" => Change::TabAdded(TabId::new(text("tab")?)),
        "tabRemoved" => Change::TabRemoved(TabId::new(text("tab")?)),
        "layoutChanged" => Change::LayoutChanged(TabId::new(text("tab")?)),
        "workspaceAdded" => Change::WorkspaceAdded(WorkspaceId::new(text("workspace")?)),
        "workspaceRemoved" => Change::WorkspaceRemoved(WorkspaceId::new(text("workspace")?)),
        "focusChanged" => Change::FocusChanged,
        "agentTransitionsMissed" => {
            Change::AgentTransitionsMissed { expected: number("expected")?, saw: number("saw")? }
        }
        other => return Err(CaseError::new(format!("no change is spelled `{other}`"))),
    })
}
