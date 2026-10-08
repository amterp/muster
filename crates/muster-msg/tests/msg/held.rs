//! What waits on a member, and what Muster may do about it: the facts a host's watch on stalled
//! groups is built from, with no daemon.

use muster_msg::{Activity, Caller, MUSTER, Memory, Messaging, Participant, Presence, Via};

/// Every agent is in its pane and idle.
struct Idle;

impl Presence for Idle {
    fn alive(&self, _: &Participant) -> bool {
        true
    }

    fn activity(&self, _: &Participant) -> Option<Activity> {
        Some(Activity::Idle)
    }

    fn agent_in(&self, _: &str) -> bool {
        true
    }

    fn has_pane(&self, _: &str) -> bool {
        true
    }
}

fn in_pane(pane: &str) -> Caller {
    Caller { pane: Some(pane.to_string()), ..Caller::default() }
}

/// `director` in `p1` and `worker` in `p2`, both in `crew`, the director having posted once.
fn crew() -> Messaging<Memory> {
    let mut messaging = Messaging::new(Memory::default());
    messaging.join(&in_pane("p1"), Some("director"), Some("crew"), &Idle, 1).unwrap();
    messaging.join(&in_pane("p2"), Some("worker"), Some("crew"), &Idle, 2).unwrap();
    messaging.post(&in_pane("p1"), None, &["worker".to_string()], "a brief", &Idle, 3).unwrap();
    messaging
}

/// A member a message waits on is held, from its oldest unread message, naming who posted; the
/// member that posted, having read everything, is not.
#[test]
fn a_member_with_a_message_unread_is_held_from_the_oldest() {
    let mut messaging = crew();
    messaging.post(&in_pane("p1"), None, &["worker".to_string()], "and another", &Idle, 9).unwrap();

    let holding = messaging.holding();
    assert_eq!(holding.len(), 1, "{holding:?}");
    let held = &holding[0];
    assert_eq!(
        (held.name.as_str(), held.pane.as_str(), held.group.as_str()),
        ("worker", "p2", "crew")
    );
    assert_eq!(held.first_at_ms, 3, "{held:?}");
    assert_eq!(held.authors, ["director"]);
}

/// What Muster posts reaches only whom it names, holds nobody, and is held to neither the
/// group's policy nor the guard that keeps a member from posting past what it has not read.
#[test]
fn what_muster_says_reaches_whom_it_names_and_holds_nobody() {
    let mut messaging = crew();
    let posted = messaging
        .post_as_muster("crew", vec!["director".to_string()], "worker has not read #4", &Idle, 20)
        .unwrap();
    assert_eq!(posted.author, MUSTER);
    let reached: Vec<&str> = posted.reached.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(reached, ["director"]);

    let holding = messaging.holding();
    assert!(
        holding.iter().all(|held| held.name != "director"),
        "the director waits on Muster's message, which nobody stalls on: {holding:?}"
    );
    assert!(messaging.post_as_muster("crew", vec![], " ", &Idle, 21).is_err(), "an empty body");
}

/// A member whose wake never landed - no longer counted woken - is woken once again, and not a
/// second time while that wake stands.
#[test]
fn a_wake_that_never_landed_is_sent_again_once() {
    let mut messaging = crew();
    assert!(messaging.wake_again("worker", "crew", &Idle).is_none(), "still counted woken");

    messaging.unwake("worker", "crew");
    let wake = messaging.wake_again("worker", "crew", &Idle).expect("woken again");
    assert_eq!(wake.via, Via::Pane("p2".to_string()));
    assert_eq!(wake.notice.from, ["director"]);
    assert!(messaging.wake_again("worker", "crew", &Idle).is_none(), "woken twice");
}
