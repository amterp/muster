//! What a closing reason means, judged against what herdr actually says. Cases live in
//! corpus/conformance/bridge-report.json.

use conformance::{CaseError, Conformance};
use muster_herdr::bridge_report;
use serde_json::{Value, json};

#[test]
fn bridge_report_conformance() {
    let corpus = Conformance::load("bridge-report.json");

    let ran = corpus.run(|given| {
        let reason = match given.get("reason") {
            None => return Err(CaseError::new("every case names a `reason`, null included")),
            Some(Value::Null) => None,
            Some(Value::String(reason)) => Some(reason.as_str()),
            Some(other) => {
                return Err(CaseError::new(format!("`reason` is {other}, not a string or null")));
            }
        };
        Ok(json!({ "ending": bridge_report::ending(reason).as_str() }))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(
        ran > 0,
        "the bridge report corpus ran no cases, which passes without proving anything"
    );
}
