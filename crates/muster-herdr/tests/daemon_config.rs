//! What Muster's daemon is told, given what a person wrote.
//! Cases live in corpus/conformance/daemon-config.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::config;
use muster_herdr::config::herdr_configuration;
use serde_json::{Value, json};

#[test]
fn daemon_config_conformance() {
    let corpus = Conformance::load("daemon-config.json");

    let ran = corpus.run(|given| {
        let lines = given.get("file").and_then(Value::as_array).ok_or_else(|| {
            CaseError::new("`file` is missing: there is no config for a daemon to be derived from")
        })?;
        let text = lines
            .iter()
            .map(|line| line.as_str().unwrap_or_default())
            .collect::<Vec<&str>>()
            .join("\n");
        // A file that will not parse never reaches this - the core refuses it whole and the
        // window runs the settings it started with - so a case here is always a file that did.
        let parsed = config::parse(&text)
            .map_err(|refusal| CaseError::new(format!("this case's file is refused: {refusal}")))?;

        Ok(fields([("file", Some(json!(herdr_configuration(&parsed.panes))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(
        ran > 0,
        "the daemon config corpus ran no cases, which passes without proving anything"
    );
}
