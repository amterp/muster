//! Which ended bridges are worth replacing, and when to stop. Cases and their reasoning live
//! in corpus/conformance/respawn.json.
//!
//! Every case is a sequence, because the answer depends on what has already been tried and how
//! long ago: the same exit is a network blinking or a pane nobody can rescue, and only the
//! interval between exits tells them apart.

use conformance::{CaseError, Conformance, fields};
use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::respawn::{Decision, Ending, Respawns};
use serde_json::{Value, json};

#[test]
fn respawn_conformance() {
    let corpus = Conformance::load("respawn.json");

    let ran = corpus.run(|given| {
        let mut respawns = Respawns::new();
        let mut decisions = Vec::new();
        let mut last: Option<PaneKey> = None;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if let Some(pane) = step.get("ended").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                let ending = ending(step)?;
                decisions.push(json!(spell(respawns.ended(&pane, number(step, "at")?, ending))));
                last = Some(pane);
            } else if let Some(pane) = step.get("stalled").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                let asked = respawns.stalled(&pane, number(step, "at")?);
                decisions.push(json!(
                    asked.map_or_else(|| "left".to_string(), |restarts| format!("ask:{restarts}"))
                ));
                last = Some(pane);
            } else if let Some(pane) = step.get("asked").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                let restarts = respawns.asked(&pane, number(step, "at")?);
                decisions.push(json!(format!("ask:{restarts}")));
                last = Some(pane);
            } else if let Some(pane) = step.get("forgot").and_then(Value::as_str) {
                let pane = pane_key(pane)?;
                respawns.forget(&pane);
                last = Some(pane);
            } else {
                return Err(CaseError::new(format!("the step does nothing: {step}")));
            }
        }

        // The pane the last step was about unless the case names another, so a case with one
        // pane in it does not have to say which one twice.
        let about = match given.get("of").and_then(Value::as_str) {
            Some(pane) => Some(pane_key(pane)?),
            None => last,
        };

        Ok(fields([
            ("decisions", Some(Value::Array(decisions))),
            ("restarts", Some(json!(about.map_or(0, |pane| respawns.restarts(&pane))))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// `start:2`, so a case reads as a sequence of answers rather than a shape to decode.
///
/// The two steps that ask for a bridge without one having ended spell themselves at the call
/// site, as `ask:N` and `left` - they answer how many bridges the pane has been given rather
/// than which attempt in a run of failures this is, and reusing `start:N` for both would put
/// two different numbers under one word.
fn spell(decision: Decision) -> String {
    match decision {
        Decision::Start(count) => format!("start:{count}"),
        Decision::GiveUp(tried) => format!("give_up:{tried}"),
        Decision::Yield => "yield".to_string(),
    }
}

/// How the bridge ended, which every step says: the answer turns on it, and a default here
/// would let a case be silent about the one field it is really about.
fn ending(step: &Value) -> Result<Ending, CaseError> {
    step.get("ending")
        .and_then(Value::as_str)
        .and_then(Ending::parse)
        .ok_or_else(|| CaseError::new(format!("the step names no known `ending`: {step}")))
}

fn number(value: &Value, key: &str) -> Result<u64, CaseError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| CaseError::new(format!("`{key}` is not a whole number of nanoseconds")))
}

/// `local/w1:p1`, split the way `PaneKey` spells itself: at the first slash, because a daemon
/// id is Muster's own and holds none where a pane id is the backend's string.
fn pane_key(spelled: &str) -> Result<PaneKey, CaseError> {
    let (daemon, pane) = spelled.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{spelled}` is not a pane key - it wants daemon/pane"))
    })?;
    Ok(PaneKey::new(&DaemonId::new(daemon), &PaneId::new(pane)))
}
