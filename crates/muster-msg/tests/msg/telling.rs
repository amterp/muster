//! What Muster tells one participant about another (MIP-7): a helper's maker that its helper
//! needs it, and the human that a maker closed with its helpers still running. Which group it
//! lands in, who it reaches, and whether a helper has posted to its maker.

use muster_msg::{Caller, HUMAN, MUSTER, Memory, Messaging, Participant, Presence, Refusal, What};

/// `p1` is the integrator and `p2` its helper maple, each with an agent in it; `p3` is critic.
/// Any other pane is closed.
struct Open;

impl Open {
    fn called(pane: &str) -> Option<&'static str> {
        match pane {
            "p1" => Some("integrator"),
            "p2" => Some("maple"),
            "p3" => Some("critic"),
            _ => None,
        }
    }
}

impl Presence for Open {
    fn alive(&self, participant: &Participant) -> bool {
        participant.pane.as_deref().and_then(Open::called).is_some()
    }

    fn agent_in(&self, pane: &str) -> bool {
        Open::called(pane).is_some()
    }

    fn name_of(&self, pane: &str) -> Option<String> {
        Open::called(pane).map(str::to_string)
    }

    fn attended(&self) -> bool {
        true
    }
}

fn in_pane(pane: &str) -> Caller {
    Caller { pane: Some(pane.to_string()), ..Caller::default() }
}

fn join(messaging: &mut Messaging<Memory>, pane: &str, group: &str) {
    let name = Open::called(pane).unwrap();
    messaging.join(&in_pane(pane), Some(name), Some(group), &Open, 1).unwrap();
}

fn members(messaging: &Messaging<Memory>, group: &str) -> Vec<String> {
    let found = messaging.groups().into_iter().find(|summary| summary.name == group);
    found.map(|summary| summary.members).unwrap_or_default()
}

/// With no group between them, Muster makes the one a post from the helper to its maker would,
/// named for what the two panes are called, and wakes only the maker.
#[test]
fn a_maker_is_told_in_a_group_made_for_the_two_when_they_share_none() {
    let mut messaging = Messaging::new(Memory::default());
    let posted = messaging.tell_as_muster("p1", "p2", None, "maple is blocked", &Open, 10).unwrap();
    assert_eq!(posted.author, MUSTER);
    assert_eq!(posted.group, "integrator+maple");
    assert_eq!(members(&messaging, "integrator+maple"), ["p1", "p2"]);
    let reached: Vec<&str> = posted.reached.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(reached, ["p1"]);
}

/// A group of exactly the two is taken over any larger group they share; one larger group
/// shared alone is taken as it is; and several, none of them the two's own, make the two a group.
#[test]
fn a_notice_goes_to_the_twos_own_group_then_the_one_they_share_then_a_new_pair() {
    let mut messaging = Messaging::new(Memory::default());
    for pane in ["p1", "p2", "p3"] {
        join(&mut messaging, pane, "crew");
    }
    let told = |messaging: &mut Messaging<Memory>| {
        messaging.tell_as_muster("p1", "p2", None, "maple is blocked", &Open, 20).unwrap().group
    };
    assert_eq!(told(&mut messaging), "crew", "the one group the two share");

    for pane in ["p1", "p2", "p3"] {
        join(&mut messaging, pane, "review");
    }
    assert_eq!(told(&mut messaging), "integrator+maple", "two councils, so a group of their own");
    assert_eq!(told(&mut messaging), "integrator+maple", "and that group from then on");
}

/// What Muster tells one agent alone lands in a group of that agent alone, named for it, and
/// stays out of every group it shares with anybody; the next notice goes in the same group.
#[test]
fn an_agent_told_alone_is_told_in_a_group_of_its_own() {
    let mut messaging = Messaging::new(Memory::default());
    join(&mut messaging, "p1", "crew");
    join(&mut messaging, "p2", "crew");
    let told = |messaging: &mut Messaging<Memory>, now| {
        messaging.tell_alone_as_muster("p2", "the wait is over", &Open, now).unwrap()
    };
    let posted = told(&mut messaging, 40);
    assert_eq!((posted.author.as_str(), posted.group.as_str()), (MUSTER, "maple"));
    assert_eq!(members(&messaging, "maple"), ["maple"]);
    assert_eq!(posted.wakes.len(), 1, "maple is woken: {:?}", posted.wakes);
    assert_eq!(told(&mut messaging, 50).group, "maple", "and that group from then on");
}

/// The human is told about a maker that closed before it ever took part in a conversation, by
/// what the maker was called.
#[test]
fn the_human_is_told_about_a_closed_pane_by_what_it_was_called() {
    let mut messaging = Messaging::new(Memory::default());
    let body = "integrator closed; 2 of its helpers are still running";
    let posted =
        messaging.tell_as_muster(HUMAN, "p9", Some("integrator"), body, &Open, 30).unwrap();
    assert_eq!(posted.group, "@human+integrator");
    assert_eq!(posted.wakes.len(), 1, "the human is woken: {:?}", posted.wakes);
    let log = messaging.log("@human+integrator", 0).unwrap();
    assert!(
        log.iter()
            .any(|entry| matches!(&entry.what, What::Message { author, .. } if author == MUSTER))
    );
}

/// A helper has posted to its maker when it addressed the maker, or posted unaddressed in a
/// group the maker is in, and not when it addressed somebody else.
#[test]
fn a_post_to_the_maker_is_found_and_one_to_somebody_else_is_not() {
    let mut messaging = Messaging::new(Memory::default());
    for pane in ["p1", "p2", "p3"] {
        join(&mut messaging, pane, "crew");
    }
    assert_eq!(messaging.last_post("p2", "p1"), None, "nothing yet");

    let to = |name: &str| vec![name.to_string()];
    messaging.post(&in_pane("p2"), Some("crew"), &to("critic"), "a question", &Open, 40).unwrap();
    assert_eq!(messaging.last_post("p2", "p1"), None, "to somebody else");

    messaging.read(&in_pane("p2"), None, &Open).ok();
    messaging
        .post(&in_pane("p2"), Some("crew"), &to("integrator"), "my report", &Open, 50)
        .unwrap();
    assert_eq!(messaging.last_post("p2", "p1"), Some(50));

    messaging.read(&in_pane("p2"), None, &Open).ok();
    messaging.post(&in_pane("p2"), Some("crew"), &[], "done", &Open, 60).unwrap();
    assert_eq!(messaging.last_post("p2", "p1"), Some(60), "unaddressed, in the maker's group");
    assert_eq!(messaging.last_post("p1", "p2"), None, "the maker has posted nothing");
}

/// What Muster tells a maker about its helper does not hold the helper's next post, since the
/// helper needs nothing in it before reporting. What Muster tells the helper itself still does.
#[test]
fn a_notice_to_the_maker_holds_no_post_of_the_helpers_and_one_to_the_helper_does() {
    let mut messaging = Messaging::new(Memory::default());
    let told = messaging.tell_as_muster("p1", "p2", None, "maple is blocked", &Open, 10).unwrap();
    let to_maker = vec!["p1".to_string()];
    messaging.post(&in_pane("p2"), Some(&told.group), &to_maker, "my report", &Open, 20).unwrap();

    messaging.tell_as_muster("p2", "p1", None, "integrator is waiting", &Open, 30).unwrap();
    let refused = messaging.post(&in_pane("p2"), Some(&told.group), &to_maker, "again", &Open, 40);
    assert_eq!(refused.unwrap_err(), Refusal::Unread { group: told.group, count: 1 });
}
