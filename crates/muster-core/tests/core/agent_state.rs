//! Agent states are the reason this project exists, so reading one wrongly is not a
//! display bug. Cases and their reasoning live in corpus/conformance/agent-state.json.

use conformance::{Conformance, fields};
use muster_core::AgentState;
use serde_json::{Value, json};

#[test]
fn agent_state_conformance() {
    let corpus = Conformance::load("agent-state.json");

    let ran = corpus.run(|given| {
        let backend = given.get("backendValue").and_then(Value::as_str).unwrap_or("");
        Ok(fields([("state", Some(json!(AgentState::from_backend(backend).as_str())))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn every_state_is_in_the_corpus() {
    // A state added to the enum and not to the corpus would be a state no case covers, in
    // the one vocabulary the whole product is about. The corpus is the definition; this
    // asserts the definition is complete rather than merely consistent.
    let corpus = Conformance::load("agent-state.json");
    let covered: Vec<&str> = corpus
        .cases
        .iter()
        .filter_map(|case| case.expect.get("state").and_then(Value::as_str))
        .collect();

    for state in AgentState::ALL {
        assert!(
            covered.contains(&state.as_str()),
            "no corpus case expects `{}`, so nothing pins how it is read",
            state.as_str()
        );
    }
}

#[test]
fn counts_as_conformance() {
    let corpus = Conformance::load("agent-state-counts-as.json");

    let ran = corpus.run(|given| {
        let word = |key: &str| given.get(key).and_then(Value::as_str).unwrap_or("");
        let (state, wanted) =
            (AgentState::from_backend(word("state")), AgentState::from_backend(word("wanted")));
        Ok(fields([("counts", Some(json!(state.counts_as(wanted))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn an_idle_agent_waiting_on_its_own_work_reads_waiting() {
    let pane = |state: &str, waiting: Option<&str>| {
        crate::support::backend::read_pane(&json!({
            "id": "p1",
            "agentState": state,
            "cwd": "/src",
            "facts": { "waiting": waiting },
        }))
        .presented_state()
    };
    assert_eq!(pane("idle", Some("the full gate")), AgentState::Waiting);
    assert_eq!(pane("idle", None), AgentState::Idle);
    // A wait is shown only once the agent has stopped: while it works, it is working.
    assert_eq!(pane("working", Some("the full gate")), AgentState::Working);
    assert_eq!(pane("blocked", Some("the full gate")), AgentState::Blocked);
}
