//! The built-in manifests, judged against the screens their authors wrote them for. Cases and
//! their reasoning live in corpus/conformance/agent-detection*.json.

use conformance::{CaseError, Conformance, fields};
use muster_detect::{Agent, Input, Manifests};
use serde_json::{Value, json};

fn detect(manifests: &Manifests, given: &Value) -> Result<Value, CaseError> {
    let text = |key: &str| given.get(key).and_then(Value::as_str).unwrap_or("");
    let agent = given
        .get("agent")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("the case names no agent"))?;
    let detection = manifests.detect(
        Some(&Agent::new(agent)),
        Input { screen: text("screen"), title: text("title"), progress: text("progress") },
    );
    Ok(fields([
        ("state", Some(json!(detection.state.as_str()))),
        ("rule", detection.rule.map(|rule| json!(rule))),
        ("visible", detection.visible.then_some(json!(true))),
        ("skipStateUpdate", detection.skip_state_update.then_some(json!(true))),
    ]))
}

#[test]
fn bundled_manifest_conformance() {
    let manifests = Manifests::built_in();
    let corpus = Conformance::load("agent-detection.json");
    let ran = corpus.run(|given| detect(&manifests, given));
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn recorded_screen_conformance() {
    let manifests = Manifests::built_in();
    let corpus = Conformance::load("agent-detection-recorded.json");
    // A recorded case knows the state the agent was in, and nothing about which rule saw it.
    let ran = corpus.run(|given| {
        let detected = detect(&manifests, given)?;
        Ok(fields([("state", detected.get("state").cloned())]))
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
