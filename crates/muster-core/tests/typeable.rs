//! Which panes have waited too long for a bridge, and what the roster owes about them. Cases
//! and their reasoning live in corpus/conformance/typeable.json.
//!
//! Every case is a sequence, because the answer depends on what has already been reported: a
//! condition that stays true has nothing new to say, and a pane that stops waiting has
//! something to take back.

use conformance::{CaseError, Conformance, fields};
use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::respawn::{Ended, Ending};
use muster_core::typeable::Waiting;
use serde_json::{Value, json};

#[test]
fn typeable_conformance() {
    let corpus = Conformance::load("typeable.json");

    let ran = corpus.run(|given| {
        let deadline = number(given, "deadline")?;
        let mut waiting = Waiting::new();
        let (mut raised, mut cleared, mut details) = (Vec::new(), Vec::new(), Vec::new());
        let mut last_read = 0;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if let Some(pane) = step.get("opened").and_then(Value::as_str) {
                waiting.opened(pane_key(pane)?, number(step, "at")?);
            } else if let Some(pane) = step.get("ended").and_then(Value::as_str) {
                waiting.ended(pane_key(pane)?, number(step, "at")?, ended(step)?);
            } else if let Some(pane) = step.get("typeable").and_then(Value::as_str) {
                waiting.typeable(&pane_key(pane)?);
            } else if let Some(pane) = step.get("closed").and_then(Value::as_str) {
                waiting.closed(&pane_key(pane)?);
            } else if let Some(now) = step.get("reconcile").and_then(Value::as_u64) {
                last_read = now;
                let reported = waiting.reconcile(now, deadline);
                for (key, detail) in reported.raise {
                    raised.push(json!(key));
                    details.push(detail);
                }
                cleared.extend(reported.clear.into_iter().map(Value::String));
            } else {
                return Err(CaseError::new(format!("the step does nothing: {step}")));
            }
        }

        // The last reading unless the case says otherwise, so that a case about what to wake
        // for next does not have to perform a reading it is not asking about.
        let now = given.get("now").and_then(Value::as_u64).unwrap_or(last_read);

        Ok(fields([
            ("raised", Some(Value::Array(raised))),
            ("cleared", Some(Value::Array(cleared))),
            ("next_wake", Some(json!(waiting.next_wake(now, deadline)))),
            // Only where a case asks for it. One sentence pinned once beats the same
            // paragraph restated in thirteen cases that are about something else.
            ("detail", given.get("detail").is_some().then(|| json!(details.last()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// How the bridge ended, which every `ended` step says: the sentence a person reads turns on
/// it, and a default here would let a case be silent about the field it is really about.
fn ended(step: &Value) -> Result<Ended, CaseError> {
    let ending = step
        .get("ending")
        .and_then(Value::as_str)
        .and_then(Ending::parse)
        .ok_or_else(|| CaseError::new(format!("the step names no known `ending`: {step}")))?;
    Ok(Ended {
        ending,
        reason: step.get("reason").and_then(Value::as_str).map(str::to_string),
        rendered: step.get("rendered").and_then(Value::as_bool).unwrap_or(false),
    })
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
