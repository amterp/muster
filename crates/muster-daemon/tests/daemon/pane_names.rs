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
}

/// A renamed pane keeps its word, and the agent list shows it beside the name, so the word stays
/// that pane's: no pane is made, renamed or drawn to it, and the pane gets it back when its name
/// is taken away.
#[test]
fn a_renamed_panes_word_stays_its_own() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let otter = word_of(&mut control, "p1");
    assert_eq!(rename(&mut control, "p1", Some("builder")).outcome(), proto::Outcome::Done);
    assert_eq!(word_of(&mut control, "p1"), otter, "a rename took the pane's word");

    make(&mut control, create("p2", in_new_tab("t2")));
    let refused = rename(&mut control, "p2", Some(&otter));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{}", refused.answer.reason);
    assert!(refused.answer.reason.contains("p1"), "names who has it: {}", refused.answer.reason);
    let made =
        pane_request::Create { label: Some(otter.clone()), ..create("p3", in_new_tab("t3")) };
    assert_eq!(control.ask(create_request(made)).outcome(), proto::Outcome::Refused);

    assert_eq!(rename(&mut control, "p1", None).outcome(), proto::Outcome::Done);
    assert_eq!(word_of(&mut control, "p1"), otter, "the pane did not get its own word back");
}

/// With every word but two taken by agents that joined messaging, one pane is drawn one of the two
/// and renamed: the next pane is drawn the other, since a renamed pane's word is not drawn again.
#[test]
fn no_pane_is_drawn_a_word_a_renamed_pane_holds() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = WORDS.split_at(WORDS.len() - 2);
    join_as(&mut control, taken);
    make(&mut control, create("p1", in_new_tab("t1")));
    let first = word_of(&mut control, "p1");
    assert_eq!(rename(&mut control, "p1", Some("builder")).outcome(), proto::Outcome::Done);
    make(&mut control, create("p2", in_new_tab("t2")));
    let second = word_of(&mut control, "p2");
    assert_ne!(second, first, "a renamed pane's word was drawn again");
    assert!(left.contains(&second.as_str()), "{second} is not one of the two left");
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

/// A post to a word would mean both a pane drawn it and an agent that joined messaging by it, so
/// no pane is drawn a word an agent here has joined by.
#[test]
fn no_pane_is_drawn_a_word_an_agent_joined_by() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = WORDS.split_at(WORDS.len() - 1);
    join_as(&mut control, taken);
    make(&mut control, create("p1", in_new_tab("t1")));
    assert_eq!(word_of(&mut control, "p1"), left[0]);
}

/// Joins one agent to messaging by each of `words`, which takes them from the words a pane can be
/// drawn: a post to one would otherwise mean both.
fn join_as(control: &mut Control, words: &[&str]) {
    use proto::msg_request::{self, Request as Asked};
    for word in words {
        let caller =
            msg_request::Caller { as_name: Some((*word).to_string()), ..Default::default() };
        let join = Asked::Join(msg_request::Join {
            name: Some((*word).to_string()),
            group: Some("crew".to_string()),
            pull: false,
        });
        let asked = proto::request::Service::Msg(proto::MsgRequest {
            caller: Some(caller),
            request: Some(join),
        });
        expect(control, asked, proto::Outcome::Done);
    }
}
