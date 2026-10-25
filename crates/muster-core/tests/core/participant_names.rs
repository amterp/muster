//! The core's reading of participants' and groups' names against
//! corpus/conformance/participant-names.json, which the daemon's reading runs against too.

use conformance::{CaseError, Conformance, fields, strings};
use muster_core::{conversation, posting};
use serde_json::{Value, json};

#[test]
fn participant_names_conformance() {
    let corpus = Conformance::load("participant-names.json");
    let ran = corpus.run(|given| {
        if let Some(questions) = given.get("questions").and_then(Value::as_array) {
            let asked: Vec<bool> = questions
                .iter()
                .map(|question| {
                    let asker = question.get("asker").and_then(Value::as_str).unwrap_or_default();
                    conversation::asked_of_human(asker, &strings(question, "to"))
                })
                .collect();
            return Ok(json!({ "asked": asked }));
        }
        let names = strings(given, "names");
        if names.is_empty() {
            return Err(CaseError::new("a case needs `names` or `questions`"));
        }
        let split: Vec<Value> = names
            .iter()
            .map(|name| conversation::split_machine(name).map_or(Value::Null, |pair| json!(pair)))
            .collect();
        let human: Vec<bool> = names.iter().map(|name| posting::is_human(name)).collect();
        let muster: Vec<bool> = names.iter().map(|name| posting::is_muster(name)).collect();
        Ok(fields([
            ("split", Some(json!(split))),
            ("human", Some(json!(human))),
            ("muster", Some(json!(muster))),
        ]))
    });
    assert_eq!(ran, corpus.cases.len());
}
