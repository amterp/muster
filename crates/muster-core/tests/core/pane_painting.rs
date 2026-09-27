//! Which panes owe a frame, and which of them are worth saying so about. Cases and their
//! reasoning live in corpus/conformance/pane-painting.json.
//!
//! Every case is a sequence against one clock, because both answers depend on what was already
//! true: a problem is raised once for a condition that stays true, cleared when the pane paints,
//! and never raised at all while something else accounts for the silence.

use conformance::{CaseError, Conformance, fields};
use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::painting::Painting;
use serde_json::{Value, json};

/// The corpus counts in milliseconds because that is what a person writing a case thinks in;
/// `Painting` counts in whatever the caller does, and the seam hands it nanoseconds. Scaled here
/// so a case would produce the sentence the window really writes.
const PER_MILLI: u64 = 1_000_000;

#[test]
fn pane_painting_conformance() {
    let corpus = Conformance::load("pane-painting.json");

    let ran = corpus.run(|given| {
        let deadline = millis(given, "deadline")?;
        let mut painting = Painting::new();
        let (mut raised, mut cleared) = (Vec::new(), Vec::new());
        let mut now = 0u64;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if let Some(pane) = step.get("typed").and_then(Value::as_str) {
                painting.typed(&pane_key(pane)?, now);
            } else if let Some(pane) = step.get("painted").and_then(Value::as_str) {
                painting.painted(&pane_key(pane)?);
            } else if let Some(pane) = step.get("closed").and_then(Value::as_str) {
                painting.closed(&pane_key(pane)?);
            } else if let Some(pane) = step.get("explained").and_then(Value::as_str) {
                painting.explained(&pane_key(pane)?, true);
            } else if let Some(pane) = step.get("fits").and_then(Value::as_str) {
                painting.explained(&pane_key(pane)?, false);
            } else if let Some(daemon) = step.get("away").and_then(Value::as_str) {
                painting.daemon_away(&DaemonId::new(daemon), true);
            } else if let Some(daemon) = step.get("back").and_then(Value::as_str) {
                painting.daemon_away(&DaemonId::new(daemon), false);
            } else if let Some(panes) = step.get("showing").and_then(Value::as_array) {
                let mut visible = std::collections::BTreeSet::new();
                for pane in panes {
                    let named = pane.as_str().ok_or_else(|| {
                        CaseError::new(format!(
                            "`showing` holds something that is not a pane: {pane}"
                        ))
                    })?;
                    visible.insert(pane_key(named)?);
                }
                painting.showing(visible);
            } else if let Some(millis) = step.get("tick").and_then(Value::as_u64) {
                now = now.saturating_add(millis.saturating_mul(PER_MILLI));
            } else {
                return Err(CaseError::new(format!("the step does nothing: {step}")));
            }

            // After every step, because every one of these facts knocks on the thread holding
            // the real clock - so a case that reconciled only at the end would be testing a
            // window nobody runs.
            let reported = painting.reconcile(now, deadline);
            raised.extend(reported.raise.into_iter().map(|(key, _)| json!(key)));
            cleared.extend(
                reported
                    .clear
                    .into_iter()
                    .map(|(key, why)| json!({ "key": key, "because": why.as_str() })),
            );
        }

        Ok(fields([
            ("raised", Some(Value::Array(raised))),
            ("cleared", Some(Value::Array(cleared))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn millis(value: &Value, key: &str) -> Result<u64, CaseError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|millis| millis.saturating_mul(PER_MILLI))
        .ok_or_else(|| CaseError::new(format!("`{key}` is not a whole number of milliseconds")))
}

/// `local/p1w3r07bsd`, split the way `PaneKey` spells itself: at the first slash, because a
/// daemon id is Muster's own and holds none where a pane id is a name Muster minted.
fn pane_key(spelled: &str) -> Result<PaneKey, CaseError> {
    let (daemon, pane) = spelled.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{spelled}` is not a pane key - it wants daemon/pane"))
    })?;
    Ok(PaneKey::new(&DaemonId::new(daemon), &PaneId::new(pane)))
}

/// The sentence's remedy keeps the agent.
///
/// A pane that stopped painting most often has a wedged bridge, and the way to a new one that
/// leaves the agent alone is a reattach. Closing the pane gets a new bridge too, by ending what
/// is running in it - so a sentence advising that costs the reader the session they were trying
/// to rescue (kan a_2MjBI7BLr).
#[test]
fn a_pane_that_stopped_painting_is_told_how_to_get_a_bridge_back_without_losing_its_agent() {
    let pane = pane_key("local/p1w3r07bsd").expect("a pane key");
    let told = muster_core::painting::stopped(&pane, 10_000 * PER_MILLI);

    assert!(
        told.contains("muster pane reattach --pane p1w3r07bsd"),
        "the sentence should name the command that asks for a new bridge: {told}"
    );
    assert!(
        !told.to_lowercase().contains("closing this pane"),
        "closing the pane ends the agent, so the sentence may not advise it: {told}"
    );
}
