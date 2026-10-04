//! What the service asks of its store: everything it answers has been kept, and what was kept is
//! enough to pick up from.

use std::collections::BTreeMap;

use muster_msg::{
    Caller, Inbox, Memory, Messaging, Participant, Policy, Presence, Refusal, Saved, What,
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

#[test]
fn a_restored_service_has_the_same_members_cursors_and_log() {
    let mut first = Messaging::new(Memory::default());
    first.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    first.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    first.post(&session("a"), None, &[], "before the restart", &Everyone, 3).unwrap();

    let store = first.store();
    let mut logs: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for (group, entry) in &store.appended {
        logs.entry(group.clone()).or_default().push(entry.clone());
    }
    let saved = store.saved.clone().expect("the service saved its state");
    let mut second = Messaging::restore(Memory::default(), saved, logs);

    // b's unread message is still unread, so the guard still holds...
    let refused = second.post(&session("b"), None, &[], "too soon", &Everyone, 4);
    assert_eq!(refused, Err(Refusal::Unread { group: "g".to_string(), count: 1 }));
    // ...and reading it clears the way, with the members read back from the log.
    let read = second.read(&session("b"), None, &Everyone).unwrap();
    assert_eq!(read.groups[0].1.len(), 1);
    let posted = second.post(&session("b"), None, &[], "after", &Everyone, 5).unwrap();
    assert_eq!(posted.seq, 5);
    assert_eq!(posted.wakes.iter().map(|wake| wake.name.as_str()).collect::<Vec<_>>(), ["a"]);
}

/// The saved state may miss its last write, since it is written without waiting for the disk;
/// a group's policy and its pause are read back from its log, which records both.
#[test]
fn a_policy_and_a_pause_the_saved_state_missed_are_read_back_from_the_log() {
    let mut first = Messaging::new(Memory::default());
    first.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    let before: Saved = first.store().saved.clone().expect("the service saved its state");
    let only_a = Policy {
        ring: BTreeMap::from([("*".to_string(), vec!["a".to_string()])]),
        ..Policy::default()
    };
    first.group_set(&session("a"), "g", only_a.clone(), &Everyone, 2).unwrap();
    first.pause(&session("a"), "g", &Everyone, 3).unwrap();

    let mut logs: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for (group, entry) in &first.store().appended {
        logs.entry(group.clone()).or_default().push(entry.clone());
    }
    let second = Messaging::restore(Memory::default(), before, logs);

    assert_eq!(second.policy("g"), Some(&Policy { paused: true, ..only_a }));
}

#[test]
fn a_post_the_store_could_not_keep_is_refused_and_not_in_the_log() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    service.store_mut().failing = true;

    let refused = service.post(&session("a"), None, &[], "lost?", &Everyone, 3);
    assert!(matches!(refused, Err(Refusal::Store { .. })), "{refused:?}");
    let log = service.log("g", 0).unwrap();
    assert!(log.iter().all(|entry| !matches!(entry.what, What::Message { .. })), "{log:?}");
}

#[test]
fn a_read_the_store_could_not_keep_moves_no_cursor() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    service.post(&session("a"), None, &[], "keep me unread", &Everyone, 3).unwrap();

    service.store_mut().failing = true;
    let refused = service.read(&session("b"), None, &Everyone);
    assert!(matches!(refused, Err(Refusal::Store { .. })), "{refused:?}");

    service.store_mut().failing = false;
    let read = service.read(&session("b"), None, &Everyone).unwrap();
    assert_eq!(read.groups[0].1.len(), 1, "{read:?}");
}

/// What a damaged log leaves behind: a reader whose cursor points past the last entry the log
/// still holds. The next post must reach it rather than fail, and it must read that post.
#[test]
fn a_cursor_past_the_head_of_its_log_is_brought_back_to_it() {
    let mut first = Messaging::new(Memory::default());
    first.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    first.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    let mut logs: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for (group, entry) in &first.store().appended {
        logs.entry(group.clone()).or_default().push(entry.clone());
    }
    let mut saved = first.store().saved.clone().expect("the service saved its state");
    for participant in &mut saved.participants {
        if participant.name == "b" {
            participant.cursors.insert("g".to_string(), 9);
        }
    }

    let mut second = Messaging::restore(Memory::default(), saved, logs);
    let posted = second.post(&session("a"), None, &[], "after the damage", &Everyone, 4).unwrap();
    assert_eq!(posted.wakes.iter().map(|wake| wake.name.as_str()).collect::<Vec<_>>(), ["b"]);
    let read = second.read(&session("b"), None, &Everyone).unwrap();
    assert_eq!(read.groups[0].1.len(), 1, "{read:?}");
}

/// Every save is a sync to the disk, under the lock every messaging request takes, and a hook
/// may run `read --if-unread` after every tool call of every agent (MIP-4, section 12).
#[test]
fn a_request_that_changes_nothing_saves_nothing() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    service.read(&session("b"), None, &Everyone).unwrap();
    let saves = service.store().saves;

    service.read(&session("b"), None, &Everyone).unwrap();
    service.join(&session("b"), Some("b"), Some("g"), &Everyone, 3).unwrap();
    let waited = service.wait(&session("b"), None, false, &Everyone).unwrap();
    assert!(matches!(waited, muster_msg::Waited::Waiting { .. }), "{waited:?}");
    assert_eq!(service.store().saves, saves);
}

/// A delete removes the log before it saves, so a crash between the two leaves the group's
/// policy and its members' places with no log. A restore keeps neither: the group stays deleted,
/// and does not come back with the default policy.
#[test]
fn a_group_whose_log_was_removed_before_a_crash_stays_deleted() {
    let mut first = Messaging::new(Memory::default());
    first.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    first.join(&session("a"), Some("a"), Some("kept"), &Everyone, 2).unwrap();
    let saved = first.store().saved.clone().expect("the service saved its state");
    let mut logs: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for (group, entry) in &first.store().appended {
        if group != "g" {
            logs.entry(group.clone()).or_default().push(entry.clone());
        }
    }

    let second = Messaging::restore(Memory::default(), saved, logs);
    assert_eq!(second.log("g", 0), Err(Refusal::NoSuchGroup { group: "g".to_string() }));
    let cursors = &second.participant("a").unwrap().cursors;
    assert_eq!(cursors.keys().collect::<Vec<_>>(), ["kept"]);
}

/// Once the log is gone the group is deleted, so a save that fails after it does not keep the
/// delete from ending the waits kept to the group: it says why, for the host to log.
#[test]
fn a_delete_whose_save_fails_still_ends_the_waits_kept_to_its_group() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    service.join(&session("b"), Some("b"), Some("g"), &Everyone, 2).unwrap();
    service.read(&session("b"), None, &Everyone).unwrap();
    let waited = service.wait(&session("b"), Some("g"), false, &Everyone).unwrap();
    let muster_msg::Waited::Waiting { ticket, .. } = waited else { panic!("{waited:?}") };

    service.store_mut().failing_saves = true;
    let deleted = service.group_delete(&session("a"), "g", &Everyone).unwrap();
    assert_eq!(deleted.ended, [ticket]);
    assert!(deleted.unsaved.is_some(), "{deleted:?}");
    assert_eq!(service.log("g", 0), Err(Refusal::NoSuchGroup { group: "g".to_string() }));
}
