//! Agent states are the reason this project exists, so reading one wrongly is not a
//! display bug. Cases and their reasoning live in corpus/conformance/agent-state.json.

use conformance::{Conformance, fields};
use muster_core::{AgentState, Until};
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

#[test]
fn a_wait_on_context_is_met_by_a_report_at_least_that_full() {
    let until = Until::parse(&["idle".to_string()], Some(80.0)).unwrap();
    assert!(until.is_wait());
    assert!(until.met(AgentState::Working, Some(80.0)));
    assert!(until.met(AgentState::Working, Some(93.5)));
    assert!(!until.met(AgentState::Working, Some(79.9)));
    // A harness that never says its context meets it only by its state.
    assert!(!until.met(AgentState::Working, None));
    assert!(until.met(AgentState::Done, None));
    assert_eq!(until.spelled(), "idle or at 80% context");

    let context_alone = Until::parse(&[], Some(50.0)).unwrap();
    assert!(context_alone.is_wait());
    assert!(!context_alone.met(AgentState::Idle, Some(10.0)));
    assert!(!Until::parse(&[], None).unwrap().is_wait());
}

#[test]
fn a_wait_nothing_can_meet_is_refused() {
    let refused = |words: &[&str], context: Option<f32>| {
        let words: Vec<String> = words.iter().map(ToString::to_string).collect();
        Until::parse(&words, context).unwrap_err()
    };
    assert!(refused(&["idel"], None).contains("`idel` is not a state"));
    assert!(refused(&[], Some(0.0)).contains("more than 0"));
    assert!(refused(&[], Some(101.0)).contains("at most 100"));
    assert!(refused(&[], Some(f32::NAN)).contains("percent"));
}
