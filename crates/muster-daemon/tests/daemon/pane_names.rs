//! What a pane is called: the callsign its daemon drew, which no other live pane on the daemon
//! shares, and the name somebody gave it, if anybody did. Either says which pane it is.

use muster_core::callsigns::CALLSIGNS;

use crate::support::*;
use proto::pane_request;

fn callsign_of(control: &mut Control, pane: &str) -> String {
    let record = snapshot(control).panes.into_iter().find(|record| record.pane == pane);
    record.and_then(|record| record.callsign).unwrap_or_else(|| panic!("{pane} has no callsign"))
}

fn rename(control: &mut Control, which: &str, label: Option<&str>) -> Asked {
    let rename = pane_request::Rename { pane: which.to_string(), label: label.map(str::to_string) };
    control.ask(pane(pane_request::Request::Rename(rename)))
}

#[test]
fn every_pane_is_drawn_a_callsign_no_other_pane_here_has() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for (at, name) in ["p2", "p3", "p4", "p5"].into_iter().enumerate() {
        let neighbour = if at == 0 { "p1".to_string() } else { format!("p{}", at + 1) };
        make(&mut control, create(name, beside(&neighbour, proto::Side::Right)));
    }
    let words: Vec<String> =
        ["p1", "p2", "p3", "p4", "p5"].iter().map(|pane| callsign_of(&mut control, pane)).collect();
    for word in &words {
        assert!(CALLSIGNS.contains(&word.as_str()), "{word} is not one of the callsigns");
    }
    let mut distinct = words.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), words.len(), "two panes share a callsign: {words:?}");
}

/// A given name is a label, and two panes may share one; what no pane may be named is another
/// pane's callsign, which an address is matched to first.
#[test]
fn panes_may_share_a_name_but_not_take_another_panes_callsign() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let named = |pane: &str, tab: &str| pane_request::Create {
        label: Some("builder".to_string()),
        ..create(pane, in_new_tab(tab))
    };
    make(&mut control, named("p1", "t1"));
    make(&mut control, named("p2", "t2"));

    let otter = callsign_of(&mut control, "p1");
    make(&mut control, create("p3", in_new_tab("t3")));
    let refused = rename(&mut control, "p3", Some(&otter));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{}", refused.answer.reason);
    assert!(refused.answer.reason.contains("p1"), "names who has it: {}", refused.answer.reason);
    assert_eq!(rename(&mut control, "p3", Some("builder")).outcome(), proto::Outcome::Done);
}

/// A renamed pane keeps its callsign, and the agent list shows it beside the name, so the callsign
/// stays that pane's: no pane is made, renamed or drawn to it, and the pane gets it back when its
/// name is taken away.
#[test]
fn a_renamed_panes_callsign_stays_its_own() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let otter = callsign_of(&mut control, "p1");
    assert_eq!(rename(&mut control, "p1", Some("builder")).outcome(), proto::Outcome::Done);
    assert_eq!(callsign_of(&mut control, "p1"), otter, "a rename took the pane's word");

    make(&mut control, create("p2", in_new_tab("t2")));
    let refused = rename(&mut control, "p2", Some(&otter));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "{}", refused.answer.reason);
    assert!(refused.answer.reason.contains("p1"), "names who has it: {}", refused.answer.reason);
    let made =
        pane_request::Create { label: Some(otter.clone()), ..create("p3", in_new_tab("t3")) };
    assert_eq!(control.ask(create_request(made)).outcome(), proto::Outcome::Refused);

    assert_eq!(rename(&mut control, "p1", None).outcome(), proto::Outcome::Done);
    assert_eq!(callsign_of(&mut control, "p1"), otter, "the pane did not get its own word back");
}

/// With every callsign but two taken by agents that joined messaging, one pane is drawn one of the
/// two and renamed: the next pane is drawn the other, since a renamed pane's callsign is not drawn
/// again.
#[test]
fn no_pane_is_drawn_a_callsign_a_renamed_pane_holds() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = CALLSIGNS.split_at(CALLSIGNS.len() - 2);
    join_as(&mut control, taken);
    make(&mut control, create("p1", in_new_tab("t1")));
    let first = callsign_of(&mut control, "p1");
    assert_eq!(rename(&mut control, "p1", Some("builder")).outcome(), proto::Outcome::Done);
    make(&mut control, create("p2", in_new_tab("t2")));
    let second = callsign_of(&mut control, "p2");
    assert_ne!(second, first, "a renamed pane's word was drawn again");
    assert!(left.contains(&second.as_str()), "{second} is not one of the two left");
}

/// A pane made in a project whose path holds one of the two callsigns left is drawn the other, so
/// `swan` is not drawn inside a project called swan. Over a few daemons, since a draw that
/// ignored the path would still pick the right callsign half the time.
#[test]
fn a_callsign_in_the_panes_directory_is_passed_over() {
    let (taken, left) = CALLSIGNS.split_at(CALLSIGNS.len() - 2);
    for _ in 0..4 {
        let daemon = daemon();
        let mut control = daemon.connect();
        join_as(&mut control, taken);
        let project = daemon.root().join(format!("my-{}-app", left[0]));
        std::fs::create_dir_all(&project).unwrap();
        let made = pane_request::Create {
            cwd: Some(project.display().to_string()),
            ..create("p1", in_new_tab("t1"))
        };
        make(&mut control, made);
        assert_eq!(callsign_of(&mut control, "p1"), left[1], "drawn the word in its directory");
    }
}

/// A callsign goes on being passed over a while after its pane closes, so a name somebody still
/// uses for the closed pane does not reach a new one.
#[test]
fn a_closed_panes_callsign_is_passed_over() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, _) = CALLSIGNS.split_at(CALLSIGNS.len() - 2);
    join_as(&mut control, taken);
    make(&mut control, create("p1", in_new_tab("t1")));
    let first = callsign_of(&mut control, "p1");
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    make(&mut control, create("p2", in_new_tab("t2")));
    assert_ne!(callsign_of(&mut control, "p2"), first, "the closed pane's word was drawn again");
}

/// A callsign the window says another machine holds is passed over while another is free.
#[test]
fn a_callsign_another_machine_holds_is_passed_over() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = CALLSIGNS.split_at(CALLSIGNS.len() - 2);
    join_as(&mut control, taken);
    let made = pane_request::Create {
        callsigns_elsewhere: vec![left[0].to_string()],
        ..create("p1", in_new_tab("t1"))
    };
    make(&mut control, made);
    assert_eq!(callsign_of(&mut control, "p1"), left[1], "drawn another machine's callsign");
}

/// Only passed over: with no other callsign free, the callsign in the directory is drawn after all.
#[test]
fn a_passed_over_callsign_is_still_drawn_when_it_is_the_only_one() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = CALLSIGNS.split_at(CALLSIGNS.len() - 1);
    join_as(&mut control, taken);
    let project = daemon.root().join(left[0]);
    std::fs::create_dir_all(&project).unwrap();
    let made = pane_request::Create {
        cwd: Some(project.display().to_string()),
        ..create("p1", in_new_tab("t1"))
    };
    make(&mut control, made);
    assert_eq!(callsign_of(&mut control, "p1"), left[0]);
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
            command: Some(format!(
                "echo \"$MUSTER_PANE_NAME $MUSTER_CALLSIGN\" > {}",
                out.display()
            )),
            ..create(pane, in_new_tab(tab))
        };
        make(control, asked);
        until_some("the pane to say its name", || {
            std::fs::read_to_string(&out).ok().filter(|text| text.ends_with('\n'))
        })
    };
    let named = said(&mut control, "p1", "t1", Some("builder"));
    assert_eq!(named, format!("builder {}\n", callsign_of(&mut control, "p1")));
    let drawn = said(&mut control, "p2", "t2", None);
    let word = callsign_of(&mut control, "p2");
    assert_eq!(drawn, format!("{word} {word}\n"));
}

/// A post to a callsign would mean both a pane drawn it and an agent that joined messaging by it,
/// so no pane is drawn a callsign an agent here has joined by.
#[test]
fn no_pane_is_drawn_a_callsign_an_agent_joined_by() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (taken, left) = CALLSIGNS.split_at(CALLSIGNS.len() - 1);
    join_as(&mut control, taken);
    make(&mut control, create("p1", in_new_tab("t1")));
    assert_eq!(callsign_of(&mut control, "p1"), left[0]);
}

/// Joins one agent to messaging by each of `callsigns`, which takes them from the callsigns a pane
/// can be drawn: a post to one would otherwise mean both.
fn join_as(control: &mut Control, callsigns: &[&str]) {
    use proto::msg_request::{self, Request as Asked};
    for word in callsigns {
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

/// A pane remembers the names it was given before, so somebody still using one - an agent's
/// `$MUSTER_PANE_NAME` keeps the name it was started with - can be told what it is called now.
#[test]
fn a_renamed_pane_remembers_what_it_was_called() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let named =
        pane_request::Create { label: Some("kelp".to_string()), ..create("p1", in_new_tab("t1")) };
    make(&mut control, named);
    let former = |control: &mut Control| {
        let record = snapshot(control).panes.into_iter().find(|record| record.pane == "p1");
        record.expect("p1 is held").former_labels
    };

    assert_eq!(rename(&mut control, "p1", Some("🤖 A")).outcome(), proto::Outcome::Done);
    assert_eq!(former(&mut control), ["kelp"]);
    assert_eq!(rename(&mut control, "p1", None).outcome(), proto::Outcome::Done);
    assert_eq!(former(&mut control), ["kelp", "🤖 A"], "and never its callsign");
    assert_eq!(rename(&mut control, "p1", Some("kelp")).outcome(), proto::Outcome::Done);
    assert_eq!(former(&mut control), ["🤖 A"], "not the name it has now");
}
