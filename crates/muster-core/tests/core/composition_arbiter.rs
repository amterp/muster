//! Exactly one thing may reach the pane per key press. Cases live in
//! corpus/conformance/composition-arbiter.json.
//!
//! The composition here is an input method's, not the window arrangement of the same name
//! (`docs/glossary.md`).

use conformance::{Conformance, fields};
use muster_core::input::{CompositionOutcome, composition_outcome};
use serde_json::{Value, json};

#[test]
fn composition_arbiter_conformance() {
    let corpus = Conformance::load("composition-arbiter.json");

    let ran = corpus.run(|given| {
        let was_composing = given.get("wasComposing").and_then(Value::as_bool).unwrap_or(false);
        let still_composing = given.get("stillComposing").and_then(Value::as_bool).unwrap_or(false);
        // Absent and null are the same answer, and the corpus uses both: one case says the
        // method never called insertText, another that it committed nothing.
        let committed = given.get("committed").and_then(Value::as_str);

        Ok(match composition_outcome(was_composing, committed, still_composing) {
            CompositionOutcome::SendText(text) => {
                fields([("outcome", Some(json!("sendText"))), ("text", Some(json!(text)))])
            }
            CompositionOutcome::SendKey => fields([("outcome", Some(json!("sendKey")))]),
            CompositionOutcome::SendNothing => fields([("outcome", Some(json!("sendNothing")))]),
        })
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
