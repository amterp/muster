//! A pane as somebody types it. Cases live in corpus/conformance/pane-addresses.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::pane_address::{Address, Candidate, resolve};
use serde_json::{Value, json};

#[test]
fn pane_address_conformance() {
    let corpus = Conformance::load("pane-addresses.json");
    let ran = corpus.run(|given| {
        let text = |value: &Value, key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| CaseError::new(format!("{key} is missing from {value}")))
        };
        let panes = given.get("panes").and_then(Value::as_array).cloned().unwrap_or_default();
        let held: Vec<(String, String, Option<String>, String)> = panes
            .iter()
            .map(|pane| {
                let callsign = pane.get("callsign").and_then(Value::as_str).map(str::to_string);
                Ok((text(pane, "pane")?, text(pane, "name")?, callsign, text(pane, "machine")?))
            })
            .collect::<Result<_, CaseError>>()?;
        let candidates: Vec<Candidate<'_>> = held
            .iter()
            .map(|(pane, name, callsign, machine)| Candidate {
                pane,
                name: Some(name),
                callsign: callsign.as_deref(),
                machine,
            })
            .collect();
        Ok(match resolve(&text(given, "typed")?, &candidates) {
            Address::Pane(pane) => fields([("address", Some(json!(pane)))]),
            Address::AsTyped => fields([("address", Some(json!("as typed")))]),
            Address::Ambiguous(why) => fields([("refused", Some(json!(why)))]),
        })
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
