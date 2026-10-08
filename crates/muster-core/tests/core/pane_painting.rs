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
use muster_core::painting::{Painting, Reader};
use serde_json::{Value, json};

use crate::support::remedy;

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
        let mut remedies: Vec<Value> = Vec::new();
        let mut now = 0u64;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if let Some(pane) = step.get("typed").and_then(Value::as_str) {
                painting.typed(&pane_key(pane)?, now);
            } else if let Some(pane) = step.get("painted").and_then(Value::as_str) {
                painting.painted(&pane_key(pane)?);
            } else if let Some(pane) = step.get("closed").and_then(Value::as_str) {
                painting.closed(&pane_key(pane)?);
            } else if let Some(pane) = step.get("stoppedReading").and_then(Value::as_str) {
                let reader = Reader {
                    program: step.get("program").and_then(Value::as_str).unwrap_or("").to_string(),
                    pid: 4242,
                    bytes: step
                        .get("bytes")
                        .and_then(Value::as_u64)
                        .and_then(|bytes| u32::try_from(bytes).ok())
                        .unwrap_or_default(),
                };
                painting.stopped_reading(&pane_key(pane)?, Some(reader));
            } else if let Some(pane) = step.get("readingAgain").and_then(Value::as_str) {
                painting.stopped_reading(&pane_key(pane)?, None);
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
            for (key, _, offered) in reported.raise {
                raised.push(json!(key));
                remedies.push(remedy::spell(offered.as_ref()));
            }
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
            // Only where a case asks, so the cases about something else need not spell it.
            ("remedy", given.get("remedy").is_some().then(|| json!(remedies.last()))),
            // Every raise's, in order, for a case about a warning said again in other words.
            ("remedies", given.get("remedies").is_some().then(|| json!(remedies))),
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

/// A pane whose program stopped reading is told so by name, and is not told to reattach: the
/// bridge is fine, and a sentence blaming it sent somebody looking for a wedge that was never
/// there (GitHub issue #1).
#[test]
fn a_pane_whose_program_stopped_reading_names_the_program_and_not_the_bridge() {
    let pane = pane_key("local/p1w3r07bsd").expect("a pane key");
    let reader = Reader { program: "codex".to_string(), pid: 4242, bytes: 2817 };
    let told = muster_core::painting::not_reading(&pane, 10_000 * PER_MILLI, &reader);

    assert!(
        told.contains("codex (pid 4242), has stopped reading its input")
            && told.contains("2817 bytes"),
        "the sentence should name the program and what it left: {told}"
    );
    assert!(
        !told.contains("pane reattach") && !told.contains("has wedged"),
        "the bridge is fine, so the sentence may not send anybody to it: {told}"
    );
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
