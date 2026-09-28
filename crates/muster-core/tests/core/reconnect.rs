//! How long to wait before trying a connection again, and when to say so out loud. Cases and
//! their reasoning live in corpus/conformance/reconnect.json.
//!
//! Every case is a sequence, because the answer depends on what has already been tried and on
//! whether anything in between actually held: a connection that comes up and falls over is the
//! same run of failures continuing, and treating it as a success is what made a plugged-in
//! laptop flap once a second for two minutes.

use conformance::{CaseError, Conformance, fields};
use muster_core::reconnect::Attempts;
use serde_json::{Value, json};

#[test]
fn reconnect_conformance() {
    let corpus = Conformance::load("reconnect.json");

    let ran = corpus.run(|given| {
        // A long run of failures, for what reaches the run log: spelled as a count, since
        // forty-five identical steps would say nothing more.
        if let Some(failures) = given.get("failures").and_then(Value::as_u64) {
            let mut attempts = Attempts::new();
            let logged_at: Vec<Value> = (0..failures)
                .map(|_| attempts.failed())
                .filter(|retry| retry.logged)
                .map(|retry| json!(retry.attempt))
                .collect();
            return Ok(fields([("logged_at", Some(Value::Array(logged_at)))]));
        }
        let mut attempts = Attempts::new();
        let mut answers = Vec::new();
        let mut recovered = Vec::new();
        let mut reported_at: Option<u32> = None;

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if step.get("failed").and_then(Value::as_bool) == Some(true) {
                let retry = attempts.failed();
                if retry.report {
                    reported_at = Some(retry.attempt);
                }
                answers.push(json!(format!("{}:{}ms", retry.attempt, retry.after / 1_000_000)));
            } else if let Some(now) = step.get("holding").and_then(Value::as_u64) {
                recovered.push(json!(attempts.holding(now)));
            } else {
                return Err(CaseError::new(format!("the step does nothing: {step}")));
            }
        }

        Ok(fields([
            ("answers", Some(Value::Array(answers))),
            // Which attempt spoke up, rather than a flag per answer. One per run is the rule,
            // so the interesting thing is where it landed and that there was only one.
            ("reported_at", Some(json!(reported_at))),
            ("recovered", Some(Value::Array(recovered))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0, "the reconnect corpus ran no cases, which passes without proving anything");
}
