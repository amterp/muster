//! One tree over a tab spanning machines, from the copy each machine keeps. Cases and their
//! reasoning live in corpus/conformance/spanning.json.

use std::collections::{BTreeMap, BTreeSet};

use crate::support::backend::{read_copy, text};
use conformance::{Conformance, fields};
use muster_core::composition::DaemonId;
use muster_core::equalize::Evenly;
use muster_core::intent::{Branch, Side};
use muster_core::mirror::backend::PaneId;
use muster_core::spanning::{self, Change, Divider, Merged, Part, Write};
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
        if let Some(path) = given.get("divider").and_then(Value::as_str) {
            let divider = spanning::side_by_side_divider(&parts, &read_turns(path));
            return Ok(fields([(
                "divider",
                Some(json!(divider.map(|divider| match divider {
                    Divider::Column { left } => format!("column {left}"),
                    Divider::Within { daemon, path } => format!("within {daemon} {}", turns(&path)),
                }))),
            )]));
        }
        let merged: Result<Option<Merged>, String> = match given
            .get("change")
            .and_then(Value::as_str)
        {
            Some(change) => spanning::rearrange(&parts, &settled, &read_change(change)).map(Some),
            None => Ok(spanning::merge(&parts, &settled)),
        };
        let merged = match merged {
            Ok(merged) => merged,
            Err(refused) => return Ok(fields([("refused", Some(json!(refused)))])),
        };
        let held = |pane: &PaneId| parts.iter().any(|part| part.holds.contains(pane));
        let drawn = given.get("drawn").and_then(Value::as_bool).unwrap_or_default().then(|| {
            merged
                .as_ref()
                .and_then(|merged| spanning::drawn(&merged.tree, &held))
                .map_or(Value::Null, |drawn| json!(drawn.to_string()))
        });
        Ok(fields([
            ("drawn", drawn),
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

/// A change as the cases spell one: `ratio <turns> <ratio>`, with the turns `first,second` from the
/// root and `-` for the root itself; `resize <pane> <side> [fraction]`; `swap <pane> <with>`;
/// `zoom <pane>`; `even <pane> <tab|row|column>`.
fn read_change(text: &str) -> Change {
    let words: Vec<&str> = text.split_whitespace().collect();
    let number = |word: &str| word.parse::<f32>().expect("a number");
    match words.as_slice() {
        ["ratio", turns, ratio] => Change::Ratio { path: read_turns(turns), ratio: number(ratio) },
        ["resize", pane, side, rest @ ..] => Change::Resize {
            pane: PaneId::new(*pane),
            side: Side::parse(side).expect("a side"),
            fraction: rest.first().map(|fraction| number(fraction)),
        },
        ["swap", pane, with] => Change::Swap { pane: PaneId::new(*pane), with: PaneId::new(*with) },
        ["zoom", pane] => Change::Zoom { pane: PaneId::new(*pane) },
        ["even", pane, evenly] => Change::Even {
            pane: PaneId::new(*pane),
            evenly: Evenly::parse(evenly).expect("a scope"),
        },
        _ => panic!("{text} is not a change"),
    }
}

/// Turns as the cases spell them: `first,second` from the root, and `-` for the root itself.
fn read_turns(text: &str) -> Vec<Branch> {
    text.split(',')
        .filter(|turn| *turn != "-")
        .map(|turn| match turn {
            "first" => Branch::First,
            "second" => Branch::Second,
            other => panic!("{other} is not a turn"),
        })
        .collect()
}

fn turns(path: &[Branch]) -> String {
    if path.is_empty() {
        return "-".to_string();
    }
    path.iter()
        .map(|turn| match turn {
            Branch::First => "first",
            Branch::Second => "second",
        })
        .collect::<Vec<_>>()
        .join(",")
}
