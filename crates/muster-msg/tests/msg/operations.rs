//! A program's post under an operation id (MIP-10, section 5): asked again, it is never posted
//! twice, whatever happened to the group or the daemon between the two.

use muster_msg::{
    Answering, Caller, Draft, HUMAN, Inbox, Memory, Messaging, OPERATION_RETENTION_MS, Operation,
    Participant, Posted, Presence, Refusal, Saved, What,
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

fn operation(id: &str, asked: u64) -> Operation {
    Operation { client: "foreman".to_string(), id: id.to_string(), asked }
}

/// The human and an agent in `g`.
fn crew() -> Messaging<Memory> {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&Caller::default(), None, Some("g"), &Everyone, 2).unwrap();
    service
}

fn post_as_human(
    service: &mut Messaging<Memory>,
    operation: &Operation,
    body: &str,
    now_ms: u64,
) -> Result<Posted, Refusal> {
    let draft = Draft { group: Some("g"), body, operation: Some(operation), ..Draft::default() };
    service.post_draft(&Caller::default(), &draft, &Everyone, now_ms)
}

fn messages(service: &Messaging<Memory>) -> usize {
    let entries = service.entries("g").unwrap_or_default();
    entries.iter().filter(|entry| matches!(entry.what, What::Message { .. })).count()
}

#[test]
fn a_repeat_is_answered_with_the_first_post_and_posts_nothing() {
    let mut service = crew();
    let first = post_as_human(&mut service, &operation("x", 7), "ship it", 10).unwrap();
    assert!(!first.already);
    let entry = service.entries("g").unwrap().last().unwrap().clone();
    let What::Message { author, via, operation: kept, .. } = entry.what else { panic!() };
    assert_eq!((author.as_str(), via.as_deref()), (HUMAN, Some("foreman")));
    assert_eq!(kept.map(|kept| kept.id), Some("x".to_string()));

    let again = post_as_human(&mut service, &operation("x", 7), "ship it", 11).unwrap();
    assert_eq!((again.already, again.group.as_str(), again.seq), (true, "g", first.seq));
    assert!(again.wakes.is_empty());
    assert_eq!(messages(&service), 1);
}

#[test]
fn an_id_used_for_another_request_is_refused_and_another_program_has_its_own() {
    let mut service = crew();
    post_as_human(&mut service, &operation("x", 7), "ship it", 10).unwrap();
    let reused = post_as_human(&mut service, &operation("x", 8), "hold it", 11);
    assert_eq!(reused, Err(Refusal::OperationReused { id: "x".to_string() }));

    let other = Operation { client: "scribe".to_string(), ..operation("x", 8) };
    assert!(!post_as_human(&mut service, &other, "hold it", 12).unwrap().already);
    assert_eq!(messages(&service), 2);
}

/// The state file is written after the log, so a crash between them leaves only the log, which
/// is enough.
#[test]
fn the_log_alone_remembers_an_id_after_a_restart() {
    let mut service = crew();
    let first = post_as_human(&mut service, &operation("x", 7), "ship it", 10).unwrap();

    let mut restored =
        Messaging::restore(Memory::default(), Saved::default(), service.store().logs());
    let again = post_as_human(&mut restored, &operation("x", 7), "ship it", 11).unwrap();
    assert_eq!((again.already, again.seq), (true, first.seq));
}

/// Deleting the group keeps the ids of its posts, in memory and in what its retired log keeps.
#[test]
fn an_id_outlives_its_group() {
    let mut service = crew();
    let first = post_as_human(&mut service, &operation("x", 7), "ship it", 10).unwrap();
    let deleting = Caller { at_ms: 20, ..session("a") };
    service.group_delete(&deleting, "g", &Everyone).unwrap();

    let again = post_as_human(&mut service, &operation("x", 7), "ship it", 21).unwrap();
    assert_eq!((again.already, again.group.as_str(), again.seq), (true, "g", first.seq));

    let mut restored =
        Messaging::restore(Memory::default(), Saved::default(), service.store().logs());
    let again = post_as_human(&mut restored, &operation("x", 7), "ship it", 22).unwrap();
    assert_eq!((again.already, again.seq), (true, first.seq));
}

#[test]
fn an_id_is_forgotten_a_day_after_its_post() {
    let mut service = crew();
    let first = post_as_human(&mut service, &operation("x", 7), "ship it", 10).unwrap();
    let later = 10 + OPERATION_RETENTION_MS;
    let again = post_as_human(&mut service, &operation("x", 7), "ship it", later).unwrap();
    assert!(!again.already);
    assert_eq!(again.seq, first.seq + 1);
}

/// A reply refused for its question leaves nothing behind, not even the group of the two that
/// the post would have made.
#[test]
fn a_reply_to_a_new_pair_is_refused_before_the_pair_is_made() {
    let mut service = crew();
    service.join(&session("b"), Some("b"), Some("other"), &Everyone, 3).unwrap();
    let groups = service.groups().len();
    let answering = Answering { question: 4, answer: "Yes".to_string() };
    let to = ["b".to_string()];
    let draft = Draft { to: &to, body: "Yes", answering: Some(&answering), ..Draft::default() };
    let refused = service.post_draft(&Caller::default(), &draft, &Everyone, 4);
    assert!(matches!(refused, Err(Refusal::NoSuchQuestion { question: 4, .. })), "{refused:?}");
    assert_eq!(service.groups().len(), groups);
}
