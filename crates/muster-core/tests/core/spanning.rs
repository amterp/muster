//! One tree over a tab spanning machines, from the copy each machine keeps. Cases and their
//! reasoning live in corpus/conformance/spanning.json.

use std::collections::{BTreeMap, BTreeSet};

use crate::support::backend::{read_copy, text};
use conformance::{Conformance, fields};
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::spanning::{self, Part, Write};
use serde_json::{Value, json};

#[test]
fn spanning_conformance() {
    let corpus = Conformance::load("spanning.json");

    let ran = corpus.run(|given| {
        let parts: Vec<Part> =
            given.get("parts").and_then(Value::as_array).into_iter().flatten().map(part).collect();
        let settled: BTreeMap<DaemonId, bool> = given
            .get("settled")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(daemon, settled)| (DaemonId::new(daemon), settled.as_bool().unwrap_or_default()))
            .collect();
        let merged = spanning::merge(&parts, &settled);
        Ok(fields([
            (
                "tree",
                Some(merged.as_ref().map_or(Value::Null, |merged| json!(merged.tree.to_string()))),
            ),
            (
                "zoomed",
                Some(
                    merged
                        .as_ref()
                        .and_then(|merged| merged.zoomed.as_ref())
                        .map_or(Value::Null, |zoomed| json!(zoomed.as_str())),
                ),
            ),
            (
                "writes",
                Some(json!(
                    merged
                        .iter()
                        .flat_map(|merged| &merged.writes)
                        .map(described)
                        .collect::<Vec<String>>()
                )),
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn part(given: &Value) -> Part {
    Part {
        daemon: DaemonId::new(text(given, "daemon")),
        copy: read_copy(&text(given, "copy")),
        zoomed: given.get("zoomed").and_then(Value::as_str).map(PaneId::new),
        generation: given.get("generation").and_then(Value::as_u64).unwrap_or_default(),
        holds: given
            .get("holds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PaneId::new)
            .collect::<BTreeSet<PaneId>>(),
        #[expect(clippy::cast_possible_truncation, reason = "a weight is a small share")]
        weight: given.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
        writable: given.get("writable").and_then(Value::as_bool).unwrap_or(true),
    }
}

/// `devenv 2<-1 <copy>`: where it goes, the generation it leaves, the one it was worked out
/// from, and the copy, with the zoom after it when there is one.
fn described(write: &Write) -> String {
    let zoomed =
        write.zoomed.as_ref().map(|zoomed| format!(" zoomed {zoomed}")).unwrap_or_default();
    format!("{} {}<-{} {}{zoomed}", write.daemon, write.generation, write.based_on, write.root)
}
