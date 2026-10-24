//! A pane as somebody types it. Cases live in corpus/conformance/pane-addresses.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::pane_address::{Address, Candidate, resolve};
use serde_json::{Value, json};

/// A pane as a case gives it.
struct Held {
    pane: String,
    name: String,
    callsign: Option<String>,
    machine: String,
    former: Vec<String>,
}

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
        let held: Vec<Held> = panes
            .iter()
            .map(|pane| {
                Ok(Held {
                    pane: text(pane, "pane")?,
                    name: text(pane, "name")?,
                    callsign: pane.get("callsign").and_then(Value::as_str).map(str::to_string),
                    machine: text(pane, "machine")?,
                    former: pane
                        .get("formerly")
                        .and_then(Value::as_array)
                        .map(|names| {
                            names.iter().filter_map(Value::as_str).map(str::to_string).collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<_, CaseError>>()?;
        let candidates: Vec<Candidate<'_>> = held
            .iter()
            .map(|held| Candidate {
                pane: &held.pane,
                name: Some(&held.name),
                callsign: held.callsign.as_deref(),
                machine: &held.machine,
                former: &held.former,
            })
            .collect();
        Ok(match resolve(&text(given, "typed")?, &candidates) {
            Address::Pane(pane) => fields([("address", Some(json!(pane)))]),
            Address::AsTyped => fields([("address", Some(json!("as typed")))]),
            Address::Ambiguous(why) => fields([("refused", Some(json!(why)))]),
            Address::Renamed(why) => fields([("renamed", Some(json!(why)))]),
        })
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
