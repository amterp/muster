//! What Muster's daemon is entitled to carry from whoever launched Muster.
//! Cases live in corpus/conformance/daemon-environment.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_herdr::daemon::carried;
use serde_json::{Value, json};

#[test]
fn daemon_environment_conformance() {
    let corpus = Conformance::load("daemon-environment.json");

    let ran = corpus.run(|given| {
        let raw = given.get("env").and_then(Value::as_object).ok_or_else(|| {
            CaseError::new("`env` is missing: there is nothing for the daemon to inherit")
        })?;
        let mut environment = BTreeMap::new();
        for (name, value) in raw {
            environment.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
        }

        let carried = carried(&environment);
        // Both halves, because a case about a leak is a case about what was *not* carried, and
        // an expectation that only listed the survivors would pass just as well if the filter
        // let everything through and the case happened to name every variable.
        Ok(fields([
            (
                "carried",
                Some(json!(
                    carried
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect::<Vec<String>>()
                )),
            ),
            (
                "dropped",
                Some(json!(
                    environment
                        .keys()
                        .filter(|name| !carried.contains_key(*name))
                        .cloned()
                        .collect::<Vec<String>>()
                )),
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
