//! What a pane is called: the name somebody gave it, or a word its daemon drew, which no other
//! live pane on the daemon shares, so a name says which pane it is.

use muster_core::words::WORDS;

use crate::support::*;
use proto::pane_request;

fn word_of(control: &mut Control, pane: &str) -> String {
    let record = snapshot(control).panes.into_iter().find(|record| record.pane == pane);
    record.and_then(|record| record.word).unwrap_or_else(|| panic!("{pane} has no word"))
}

fn rename(control: &mut Control, which: &str, label: Option<&str>) -> Asked {
    let rename = pane_request::Rename { pane: which.to_string(), label: label.map(str::to_string) };
    control.ask(pane(pane_request::Request::Rename(rename)))
}

#[test]
fn every_pane_is_drawn_a_word_no_other_pane_here_has() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for (at, name) in ["p2", "p3", "p4", "p5"].into_iter().enumerate() {
        let neighbour = if at == 0 { "p1".to_string() } else { format!("p{}", at + 1) };
        make(&mut control, create(name, beside(&neighbour, proto::Side::Right)));
    }
    let words: Vec<String> =
        ["p1", "p2", "p3", "p4", "p5"].iter().map(|pane| word_of(&mut control, pane)).collect();
    for word in &words {
        assert!(WORDS.contains(&word.as_str()), "{word} is not one of the words");
    }
    let mut distinct = words.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), words.len(), "two panes share a word: {words:?}");
}

#[test]
fn a_pane_cannot_be_made_or_renamed_to_a_name_another_pane_has() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let named = pane_request::Create {
        label: Some("builder".to_string()),
        ..create("p1", in_new_tab("t1"))
    };
    make(&mut control, named);
    let otter = word_of(&mut control, "p1");

    let again = pane_request::Create {
        label: Some("builder".to_string()),
        ..create("p2", in_new_tab("t2"))
    };
    let refused = control.ask(create_request(again));
    assert_eq!(refused.outcome(), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("p1"), "names who has it: {}", refused.answer.reason);

    make(&mut control, create("p2", in_new_tab("t2")));
    let refused = rename(&mut control, "p2", Some("builder"));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{}", refused.answer.reason);
    // p1 goes by its label, so its word is not its name, and another pane may take it.
    assert_eq!(rename(&mut control, "p2", Some(&otter)).outcome(), proto::Outcome::Done);

    // Back to its word, which p2 is called now: p1 is drawn another.
    assert_eq!(rename(&mut control, "p1", None).outcome(), proto::Outcome::Done);
    let redrawn = word_of(&mut control, "p1");
    assert_ne!(redrawn, otter);
    assert!(WORDS.contains(&redrawn.as_str()));
}

#[test]
fn a_closed_panes_name_is_free_again() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let named = |pane: &str, tab: &str| pane_request::Create {
        label: Some("critic".to_string()),
        ..create(pane, in_new_tab(tab))
    };
    make(&mut control, named("p1", "t1"));
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    make(&mut control, named("p2", "t2"));
}

#[test]
fn a_pane_is_told_what_it_is_called() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let said = |control: &mut Control, pane: &str, tab: &str, label: Option<&str>| {
        let out = daemon.root().join(pane);
        let asked = pane_request::Create {
            label: label.map(str::to_string),
            command: Some(format!("echo \"$MUSTER_PANE_NAME\" > {}", out.display())),
            ..create(pane, in_new_tab(tab))
        };
        make(control, asked);
        until_some("the pane to say its name", || {
            std::fs::read_to_string(&out).ok().filter(|text| text.ends_with('\n'))
        })
    };
    assert_eq!(said(&mut control, "p1", "t1", Some("builder")), "builder\n");
    let drawn = said(&mut control, "p2", "t2", None);
    assert_eq!(drawn.trim_end(), word_of(&mut control, "p2"));
}
