//! Which agents have been seen, and so which are `done`. Cases and their reasoning live in
//! corpus/conformance/attention.json.
//!
//! Every case is a fold over a sequence, because seen-ness is one: what a transition means
//! depends on what the window was showing when it arrived.

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields};
use muster_core::AgentState;
use muster_core::attention::Attention;
use muster_core::composition::{DaemonId, PaneKey};
use muster_core::mirror::backend::PaneId;
use serde_json::{Map, Value, json};

#[test]
fn attention_conformance() {
    let corpus = Conformance::load("attention.json");

    let ran = corpus.run(|given| {
        let mut attention = Attention::new();
        if let Some(focused) = given.get("focused").and_then(Value::as_bool) {
            attention.window_focused(focused);
        }
        attention.showing(read_panes(given, "visible")?);

        // The daemon's last word per pane, tracked here rather than held by `Attention`:
        // the mirror is where a backend's state lives, and a second copy inside the thing
        // that presents it would be a second copy to disagree with.
        let mut backend: BTreeMap<PaneKey, AgentState> = BTreeMap::new();

        for event in given.get("events").and_then(Value::as_array).into_iter().flatten() {
            if let Some(focused) = event.get("focused").and_then(Value::as_bool) {
                attention.window_focused(focused);
                continue;
            }
            if event.get("visible").is_some() {
                attention.showing(read_panes(event, "visible")?);
                continue;
            }
            // A pane the backend no longer holds. Its own step because what it proves is
            // about the id rather than about a state: ids are reused, so what is remembered
            // about a closed pane is inherited by the next one to be given its name.
            if event.get("closed").is_some() {
                let pane = read_pane(event, "closed")?;
                attention.forget(&pane);
                backend.remove(&pane);
                continue;
            }
            // A pane this window is meeting for the first time, as the daemon already had
            // it - which is what every pane looks like on the way up.
            if event.get("appeared").is_some() {
                let pane = read_pane(event, "appeared")?;
                let state = read_state(event, "state")?;
                attention.first_seen(&pane, state);
                backend.insert(pane, state);
                continue;
            }
            let pane = read_pane(event, "pane")?;
            let from = read_state(event, "from")?;
            let to = read_state(event, "to")?;
            attention.observed(&pane, from, to);
            backend.insert(pane, to);
        }

        let states: Map<String, Value> = backend
            .iter()
            .map(|(pane, state)| {
                (pane.to_string(), json!(attention.presented(pane, *state).as_str()))
            })
            .collect();
        Ok(fields([("states", Some(Value::Object(states)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// `local/w1:p1` - a daemon and a pane in one token.
///
/// Split at the first slash rather than the last, because a pane id is the backend's own
/// string and Muster never parses it. A daemon id is Muster's own and holds no slash.
fn read_key(text: &str) -> Result<PaneKey, CaseError> {
    let (daemon, pane) = text.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{text}` names no daemon - a pane is written `local/w1:p1`"))
    })?;
    Ok(PaneKey { daemon: DaemonId::new(daemon), pane: PaneId::new(pane) })
}

fn read_pane(given: &Value, key: &str) -> Result<PaneKey, CaseError> {
    let text = given
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))?;
    read_key(text)
}

fn read_panes(given: &Value, key: &str) -> Result<BTreeSet<PaneKey>, CaseError> {
    given
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| {
            entry
                .as_str()
                .ok_or_else(|| {
                    CaseError::new(format!("`{key}` holds something that is not a pane"))
                })
                .and_then(read_key)
        })
        .collect()
}

fn read_state(given: &Value, key: &str) -> Result<AgentState, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(AgentState::from_backend)
        .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))
}
