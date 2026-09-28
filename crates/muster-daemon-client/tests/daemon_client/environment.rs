//! What Muster's daemon is entitled to carry from whoever launched Muster, here and on another
//! machine. Cases live in corpus/conformance/daemon-environment.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_daemon_client::environment::{carried, for_far_daemon, supplied};
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
        // A daemon on another machine, started from that machine's own environment.
        let far = given.get("far").and_then(Value::as_bool).unwrap_or(false);
        // The directory this build's `muster` command sits in, which the shell decides and puts a
        // link in. Absent is a build that staged no CLI, and then a pane's PATH is whatever it
        // inherited.
        let commands = given.get("commands").and_then(Value::as_str);

        let (carried, supplied) = if far {
            (for_far_daemon(&environment), BTreeMap::new())
        } else {
            (carried(&environment), supplied(&environment, locale, commands))
        };
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
