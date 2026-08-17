//! Which refusals mean the window is showing something that is not there, and which of a
//! daemon's successes are refusals. Cases live in corpus/conformance/daemon-refusal.json and
//! corpus/conformance/declined-intent.json.
//!
//! Both drivers here because they answer one question by two routes: a daemon that would not
//! make a change says so either as an error or as a success that changed nothing, and both end
//! in the same two-variant [`Refusal`]. Read apart, either one looks like the whole rule.

use conformance::{CaseError, Conformance, fields};
use muster_core::intent::Refusal;
use muster_herdr::client::Failure;
use muster_herdr::{considered, refusal};
use serde_json::{Value, json};

#[test]
fn daemon_refusal_conformance() {
    let corpus = Conformance::load("daemon-refusal.json");

    let ran = corpus.run(|given| {
        let answer = refusal(&failure(given)?);
        Ok(fields([("kind", Some(json!(kind(&answer)))), ("detail", Some(json!(answer.detail())))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn declined_intent_conformance() {
    let corpus = Conformance::load("declined-intent.json");

    let ran = corpus.run(|given| {
        let reason = text(given, "reason")?;
        // A `None` is the whole answer for a case whose state already held: there is no detail
        // because there is nothing to tell anybody. Spelled here rather than as an absent
        // `kind`, so that a rule accidentally answering nothing at all fails a case instead of
        // matching one.
        let Some(answer) = considered(&reason) else {
            return Ok(fields([("kind", Some(json!("already_so")))]));
        };
        Ok(fields([("kind", Some(json!(kind(&answer)))), ("detail", Some(json!(answer.detail())))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// One refusal as the corpus spells its kind.
fn kind(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::NotThere(_) => "not_there",
        Refusal::Declined(_) => "declined",
    }
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
