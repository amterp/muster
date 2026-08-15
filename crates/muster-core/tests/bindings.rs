//! What Muster binds out of the box. Cases live in corpus/conformance/bindings.json.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields};
use muster_core::input::{Action, Bindings, Chord, Modifiers};
use serde_json::{Value, json};

#[test]
fn bindings_conformance() {
    let corpus = Conformance::load("bindings.json");
    let bindings = Bindings::default();

    let ran = corpus.run(|given| {
        let wanted = given
            .get("actions")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("`actions` is missing: there is nothing to look up"))?;

        // `*` means every action, which is how the case about the whole table asks for it
        // without listing fifteen names that would then need editing to add a sixteenth.
        if wanted.iter().any(|name| name.as_str() == Some("*")) {
            let chords: BTreeSet<String> =
                bindings.all().filter_map(|(_, chord)| Some(spell(chord?))).collect();
            return Ok(fields([
                ("actions", Some(json!(bindings.all().count()))),
                ("bound", Some(json!(bindings.all().filter(|(_, on)| on.is_some()).count()))),
                ("distinct_chords", Some(json!(chords.len()))),
            ]));
        }

        let mut chords = Vec::new();
        for name in wanted {
            let name = name.as_str().unwrap_or_default();
            let action = Action::parse(name)
                .ok_or_else(|| CaseError::new(format!("`{name}` is not an action")))?;
            let chord = bindings
                .chord(action)
                .ok_or_else(|| CaseError::new(format!("`{name}` has no default chord")))?;
            chords.push(format!("{}={}", action.as_str(), spell(chord)));
        }
        Ok(fields([("chords", Some(json!(chords)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn a_config_rebinds_what_it_names_and_leaves_the_rest() {
    // The property a partial config depends on. A file that had to restate all fifteen to
    // change one is a file nobody edits twice.
    let mut bindings = Bindings::default();
    bindings.bind(Action::SplitRight, "ctrl+backslash").expect("that is a readable chord");

    assert_eq!(spell(bindings.chord(Action::SplitRight).expect("just bound")), "control+Backslash");
    assert_eq!(
        bindings.chord(Action::SplitDown),
        Bindings::default().chord(Action::SplitDown),
        "rebinding one action moved another"
    );
}

#[test]
fn an_unbound_action_keeps_its_place_and_loses_its_chord() {
    // Somebody who wants their chord back for something else has to be able to say so, and
    // the alternative is binding it to a key nobody presses. What they gave up is the
    // shortcut: the action is still published, so a shell still has a menu item to offer, and
    // on macOS an action with no item is one nothing can reach.
    let mut bindings = Bindings::default();
    bindings.unbind(Action::ClosePane);

    assert_eq!(bindings.chord(Action::ClosePane), None);
    assert_eq!(
        bindings.all().find(|(action, _)| *action == Action::ClosePane),
        Some((Action::ClosePane, None)),
        "an unbound action stopped being published at all"
    );
}

#[test]
fn the_two_splits_ghostty_leaves_unbound_ship_unbound() {
    // Parity, and the careful half of it: Ghostty has `new_split:left` and `new_split:up` and
    // ships neither on a chord. Muster shipping one would be Muster inventing a shortcut for
    // an action the terminal it embeds deliberately left alone.
    let bindings = Bindings::default();

    assert_eq!(bindings.chord(Action::SplitLeft), None);
    assert_eq!(bindings.chord(Action::SplitUp), None);
    assert!(
        bindings.all().any(|(action, chord)| action == Action::SplitLeft && chord.is_none()),
        "splitting leftward is not offered at all, so nothing can reach it"
    );
}

/// A chord as `shift+super+KeyD`: modifiers in bit order, then the key.
///
/// Bit order rather than the order somebody typed them, so the same chord spells one way
/// wherever it appears - in a case, in a log line, in a message about a collision.
fn spell(chord: Chord) -> String {
    let mut spelled: Vec<&str> = Modifiers::ALL_NAMES
        .into_iter()
        .filter(|(_, bit)| Modifiers::CHORD.contains(*bit) && chord.modifiers.contains(*bit))
        .map(|(name, _)| name)
        .collect();
    spelled.push(chord.key.as_str());
    spelled.join("+")
}
