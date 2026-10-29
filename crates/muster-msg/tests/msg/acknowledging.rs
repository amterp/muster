//! A program acknowledging the human's messages through the one it showed them (MIP-10, section
//! 5), and the human answering a question from somewhere other than the group's conversation.

use muster_msg::{
    Answering, Caller, Draft, HUMAN, Inbox, Memory, Messaging, Participant, Presence, Refusal,
};

struct Everyone;

impl Presence for Everyone {
    fn alive(&self, _: &Participant) -> bool {
        true
    }
}

fn session(name: &str) -> Caller {
    Caller {
        inbox: Some(Inbox { socket: format!("/tmp/cc-socks/{name}.sock"), inode: 1 }),
        ..Caller::default()
    }
}

/// The human and an agent in `g`, the agent having posted three messages to the human, numbered
/// from the returned one.
fn three_waiting() -> (Messaging<Memory>, u64) {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&Caller::default(), None, Some("g"), &Everyone, 2).unwrap();
    let to = [HUMAN.to_string()];
    let first = service.post(&session("a"), Some("g"), &to, "one", &Everyone, 3).unwrap().seq;
    service.post(&session("a"), Some("g"), &to, "two", &Everyone, 4).unwrap();
    service.post(&session("a"), Some("g"), &to, "three", &Everyone, 5).unwrap();
    (service, first)
}

#[test]
fn an_acknowledgement_moves_the_place_to_the_number_given_and_leaves_what_came_after() {
    let (mut service, first) = three_waiting();
    let acked = service.ack("g", first, &Everyone).unwrap();
    assert_eq!((acked.place, acked.moved), (first, true));
    assert_eq!(service.human_place("g"), Some(first));
    let notice = service.human_notice("g");
    assert_eq!((notice.count, notice.first), (2, first + 1));
}

#[test]
fn an_acknowledgement_never_moves_the_place_back() {
    let (mut service, first) = three_waiting();
    service.ack("g", first + 1, &Everyone).unwrap();
    let acked = service.ack("g", first, &Everyone).unwrap();
    assert_eq!((acked.place, acked.moved), (first + 1, false));
    assert_eq!(service.human_notice("g").count, 1);
}

#[test]
fn a_number_past_the_newest_or_a_group_without_the_human_is_refused() {
    let (mut service, first) = three_waiting();
    let refused = service.ack("g", first + 3, &Everyone);
    let past =
        Refusal::PastNewest { group: "g".to_string(), through: first + 3, newest: first + 2 };
    assert_eq!(refused, Err(past));

    service.join(&session("b"), Some("b"), Some("other"), &Everyone, 6).unwrap();
    let refused = service.ack("other", 1, &Everyone);
    assert!(matches!(refused, Err(Refusal::NotAMember { .. })), "{refused:?}");
    assert_eq!(service.human_place("g"), Some(first - 1));
}

/// Reading as the human still reads everything, whatever was acknowledged before.
#[test]
fn a_read_after_an_acknowledgement_reads_to_the_end() {
    let (mut service, first) = three_waiting();
    service.ack("g", first, &Everyone).unwrap();
    let read = service.read(&Caller::default(), Some("g"), &Everyone).unwrap();
    assert_eq!(read.groups[0].1.len(), 2);
    assert_eq!(service.human_notice("g").count, 0);
}

/// A question the human answered without reading the group still waits, and says it is answered,
/// for the windows to take its banner down.
#[test]
fn a_question_answered_without_reading_is_listed_as_answered() {
    let (mut service, _) = three_waiting();
    let (to, answers) = ([HUMAN.to_string()], ["Yes".to_string(), "No".to_string()]);
    let draft =
        Draft { group: Some("g"), to: &to, body: "Ship?", answers: &answers, ..Draft::default() };
    let question = service.post_draft(&session("a"), &draft, &Everyone, 6).unwrap().seq;
    assert!(service.human_answered("g").is_empty());

    let answering = Answering { question, answer: "Yes".to_string() };
    let to = ["a".to_string()];
    let reply = Draft {
        group: Some("g"),
        to: &to,
        body: "Answer: Yes",
        answering: Some(&answering),
        ..Draft::default()
    };
    service.post_draft(&Caller::default(), &reply, &Everyone, 7).unwrap();
    assert_eq!(service.human_answered("g"), [question]);
    assert_eq!(service.human_notice("g").count, 4);
}
