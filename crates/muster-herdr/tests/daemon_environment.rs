//! What Muster's daemon is entitled to carry from whoever launched Muster.
//! Cases live in corpus/conformance/daemon-environment.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_herdr::daemon::{carried, supplied};
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
        // What the platform said this machine's locale is, which the shell reports and only a
        // case about a GUI launch has to name. Absent is a platform that would not say.
        let locale = given.get("locale").and_then(Value::as_str);
        // Where Muster wrote the config file this daemon is to read, which only the shell can
        // answer. Absent is a launch that found nowhere to write one, and the daemon then
        // reads the user's own herdr config the way it did before this existed.
        let daemon_config = given.get("daemon_config").and_then(Value::as_str);

        let carried = carried(&environment);
        let supplied = supplied(&environment, locale, daemon_config);
        // All three, because a case about a leak is a case about what was *not* carried, and
        // an expectation that only listed the survivors would pass just as well if the filter
        // let everything through and the case happened to name every variable. `supplied` is
        // the third because a variable that was never in the environment is neither of the
        // other two.
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
                "supplied",
                Some(json!(
                    supplied
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
