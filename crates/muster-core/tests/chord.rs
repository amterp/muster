//! Reading a chord out of a config file. Cases live in corpus/conformance/chord.json.

use conformance::{CaseError, Conformance, fields};
use muster_core::input::{Chord, Modifiers};
use serde_json::{Value, json};

#[test]
fn chord_conformance() {
    let corpus = Conformance::load("chord.json");

    let ran = corpus.run(|given| {
        let text = given
            .get("chord")
            .and_then(Value::as_str)
            .ok_or_else(|| CaseError::new("`chord` is missing: there is nothing to read"))?;

        // Whether it read, and what it read into. Not the refusal's wording: prose in a
        // corpus is a corpus that fails every time the wording improves, and what the message
        // has to say is pinned below where it can be checked by what it contains.
        Ok(match Chord::parse(text) {
            Ok(chord) => fields([
                ("refused", Some(json!(false))),
                ("key", Some(json!(chord.key.as_str()))),
                ("modifiers", Some(json!(names(chord.modifiers)))),
            ]),
            Err(_) => fields([("refused", Some(json!(true)))]),
        })
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A refusal is matched by what it contains rather than in full, so the wording can improve
/// without every case needing an edit. What is pinned is that it names the thing that is
/// wrong.
#[test]
fn a_refusal_names_the_piece_it_could_not_read() {
    let refusal = Chord::parse("cmd+meta").expect_err("meta is not a key");
    assert!(refusal.contains("meta"), "the refusal should quote what it read: {refusal}");
    assert!(
        refusal.contains("left") && refusal.contains("f1"),
        "the refusal should say what is spellable, and said: {refusal}"
    );
}

/// The names Muster uses for the bits that pick a binding, in bit order.
///
/// Only the four a chord can carry. Caps lock and the left-or-right bits are reported by a
/// keyboard and are not something anybody binds.
fn names(modifiers: Modifiers) -> Vec<&'static str> {
    Modifiers::ALL_NAMES
        .into_iter()
        .filter(|(_, bit)| Modifiers::CHORD.contains(*bit) && modifiers.contains(*bit))
        .map(|(name, _)| name)
        .collect()
}
