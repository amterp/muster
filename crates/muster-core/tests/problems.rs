//! What is wrong with a window, and when saying so again is worth anybody's attention. Cases
//! and their reasoning live in corpus/conformance/problems.json.
//!
//! Every case is a sequence, because a problem is a condition rather than an event: what
//! raising one means depends on whether it was already raised, and with what.

use conformance::{CaseError, Conformance, fields};
use muster_core::problems::{Problems, Severity};
use serde_json::{Value, json};

#[test]
fn problems_conformance() {
    let corpus = Conformance::load("problems.json");

    let ran = corpus.run(|given| {
        let mut problems = Problems::new();
        let mut changed: Vec<Value> = Vec::new();

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if let Some(key) = step.get("clear").and_then(Value::as_str) {
                changed.push(json!(problems.clear(key)));
                continue;
            }
            let key = text(step, "raise")?;
            let detail = text(step, "detail")?;
            changed.push(json!(problems.raise(&key, severity(step)?, &detail)));
        }

        // Formatted rather than nested, so that a case reads as the list somebody would see and
        // a wrong order is obvious in the diff rather than three lines deep in it.
        let outstanding: Vec<Value> = problems
            .outstanding()
            .into_iter()
            .map(|problem| {
                json!(format!("{} {}: {}", problem.severity.as_str(), problem.key, problem.detail))
            })
            .collect();

        Ok(fields([
            ("changed", Some(Value::Array(changed))),
            ("outstanding", Some(Value::Array(outstanding))),
            ("has_error", Some(json!(problems.has_error()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn text(step: &Value, key: &str) -> Result<String, CaseError> {
    step.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CaseError::new(format!("the step has no `{key}`")))
}

fn severity(step: &Value) -> Result<Severity, CaseError> {
    match step.get("severity").and_then(Value::as_str) {
        Some("error") => Ok(Severity::Error),
        Some("warning") => Ok(Severity::Warning),
        other => Err(CaseError::new(format!(
            "a step's severity is {other:?} - only `error` and `warning` exist"
        ))),
    }
}
