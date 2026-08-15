//! Where Muster's own daemon listens, which is deliberately not where anybody else's does.
//! Cases live in corpus/conformance/own-socket.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_herdr::own_socket_path;
use serde_json::{Value, json};

#[test]
fn own_socket_conformance() {
    let corpus = Conformance::load("own-socket.json");

    let ran = corpus.run(|given| {
        let raw = given
            .get("env")
            .and_then(Value::as_object)
            .ok_or_else(|| CaseError::new("`env` is missing: there is nothing to resolve from"))?;
        let mut environment = BTreeMap::new();
        for (name, value) in raw {
            environment.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
        }

        // Null rather than absent, on the same terms as socket discovery: "nowhere to look"
        // is an answer some of these cases are about.
        Ok(fields([(
            "path",
            Some(own_socket_path(&environment).map_or(Value::Null, |path| json!(path))),
        )]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
