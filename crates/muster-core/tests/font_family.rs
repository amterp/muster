//! What a window says about the font family it was told to use. Cases live in
//! corpus/conformance/font-family.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::font::{FontReport, problem};
use serde_json::{Value, json};

#[test]
fn font_family_conformance() {
    let corpus = Conformance::load("font-family.json");

    let ran = corpus.run(|given| {
        let text = |key: &str| {
            given
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| CaseError::new(format!("the case names no `{key}`")))
        };
        let flag = |key: &str| {
            given
                .get(key)
                .and_then(Value::as_bool)
                .ok_or_else(|| CaseError::new(format!("the case names no `{key}`")))
        };
        let raised = problem(&FontReport {
            family: text("family")?.to_string(),
            found: flag("found")?,
            monospaced: flag("monospaced")?,
        });

        Ok(fields([
            // The severity and the key together, because which condition was raised and how
            // loudly are one decision: severity is what opens a roster somebody had closed.
            (
                "problem",
                Some(raised.as_ref().map_or(Value::Null, |problem| {
                    json!(format!("{} {}", problem.severity.as_str(), problem.key))
                })),
            ),
            ("detail", raised.as_ref().map(|problem| json!(problem.detail))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
