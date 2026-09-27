//! Where the dividers go for a set of panes to come out the same. Cases and their reasoning
//! live in corpus/conformance/equalize.json.

use crate::support::backend::{read_node, text};
use conformance::{CaseError, Conformance, fields};
use muster_core::equalize::{self, Divider, Evenly};
use muster_core::intent::Branch;
use muster_core::mirror::backend::PaneId;
use serde_json::{Value, json};

#[test]
fn equalize_conformance() {
    let corpus = Conformance::load("equalize.json");

    let ran = corpus.run(|given| {
        let root = read_node(given.get("root").unwrap_or(&Value::Null));
        let named = text(given, "evenly");
        let evenly = Evenly::parse(&named).ok_or_else(|| {
            CaseError::new(format!("the case asks to even out a `{named}`, which is not a scope"))
        })?;
        let evened = equalize::dividers(&root, &PaneId::new(text(given, "pane")), evenly);
        Ok(fields([(
            "evened",
            Some(match evened {
                // Null is a scope the pane does not sit in, which is a different answer from
                // an empty list: that one is a set of panes already even.
                None => Value::Null,
                Some(dividers) => json!(dividers.iter().map(described).collect::<Vec<String>>()),
            }),
        )]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A scope nothing evens out here is a scope nothing decides.
#[test]
fn every_scope_is_evened_out_in_the_corpus() {
    let corpus = Conformance::load("equalize.json");
    let asked: Vec<String> = corpus.cases.iter().map(|case| text(&case.given, "evenly")).collect();
    for evenly in Evenly::ALL {
        assert!(
            asked.iter().any(|named| named == evenly.as_str()),
            "no corpus case evens out a `{}`, so what that word reaches is decided by nothing",
            evenly.as_str()
        );
    }
}

/// One divider the way a case reads best: the turns down to it, then where it goes.
///
/// The same `@` a tree is written with (`muster_core::mirror::backend::LayoutNode`), and the
/// ratio at full width for the same reason - a reviewer deciding whether an expectation is
/// right can see that 0.33333334 is a third, and cannot see what a rounded one is hiding.
fn described(divider: &Divider) -> String {
    let turns: Vec<&str> = divider
        .path
        .iter()
        .map(|turn| if *turn == Branch::First { "first" } else { "second" })
        .collect();
    let path = if turns.is_empty() { "root".to_string() } else { turns.join(".") };
    format!("{path}@{}", divider.ratio)
}
