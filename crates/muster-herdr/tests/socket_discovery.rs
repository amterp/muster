//! Where a daemon listens, reimplemented from herdr's own rules rather than asked for.
//! Cases live in corpus/conformance/socket-discovery.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_herdr::discover_socket_path;
use serde_json::{Value, json};

#[test]
fn socket_discovery_conformance() {
    let corpus = Conformance::load("socket-discovery.json");

    let ran = corpus.run(|given| {
        let raw = given
            .get("env")
            .and_then(Value::as_object)
            .ok_or_else(|| CaseError::new("`env` is missing: there is nothing to discover from"))?;
        let mut environment = BTreeMap::new();
        for (name, value) in raw {
            environment.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
        }

        // Null rather than absent, because "nowhere to look" is the answer the case is
        // about and a missing key would read as a driver that forgot to report one.
        Ok(fields([(
            "path",
            Some(discover_socket_path(&environment).map_or(Value::Null, |path| json!(path))),
        )]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
