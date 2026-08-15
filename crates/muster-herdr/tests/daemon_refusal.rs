//! Which refusals mean the window is showing something that is not there. Cases live in
//! corpus/conformance/daemon-refusal.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::intent::Refusal;
use muster_herdr::client::Failure;
use muster_herdr::refusal;
use serde_json::{Value, json};

#[test]
fn daemon_refusal_conformance() {
    let corpus = Conformance::load("daemon-refusal.json");

    let ran = corpus.run(|given| {
        let answer = refusal(&failure(given)?);
        let kind = match answer {
            Refusal::NotThere(_) => "not_there",
            Refusal::Declined(_) => "declined",
        };
        Ok(fields([("kind", Some(json!(kind))), ("detail", Some(json!(answer.detail())))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// One case's `given`, as the failure it describes.
fn failure(given: &Value) -> Result<Failure, CaseError> {
    let kind = given
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("`kind` is missing: there is no failure to classify"))?;
    match kind {
        "daemon" => {
            Ok(Failure::Daemon { code: text(given, "code")?, message: text(given, "message")? })
        }
        "unreachable" => Ok(Failure::Unreachable(text(given, "detail")?)),
        "timed_out" => Ok(Failure::TimedOut),
        "malformed" => Ok(Failure::MalformedResponse),
        other => Err(CaseError::new(format!("`{other}` is not a failure this driver knows"))),
    }
}

fn text(given: &Value, key: &str) -> Result<String, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CaseError::new(format!("`{key}` is missing")))
}
