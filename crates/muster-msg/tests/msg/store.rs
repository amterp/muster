//! What the service asks of its store: everything it answers has been kept, and what was kept is
//! enough to pick up from.

use std::collections::BTreeMap;

use muster_msg::{
    Activity, Caller, HUMAN, Inbox, Memory, Messaging, Participant, Policy, Presence, Refusal,
    Saved, Via, What,
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
    let saved = store.saved.clone().expect("the service saved its state");
    let mut second = Messaging::restore(Memory::default(), saved, store.logs());

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

#[test]
fn a_restored_human_loses_agent_addresses_but_keeps_its_unread_messages() {
    struct Agent;

    impl Presence for Agent {
        fn alive(&self, _: &Participant) -> bool {
            true
        }

        fn agent_in(&self, pane: &str) -> bool {
            pane == "p1"
        }

        fn activity(&self, participant: &Participant) -> Option<Activity> {
            participant
                .pane
                .as_deref()
                .filter(|pane| self.agent_in(pane))
                .map(|_| Activity::Working)
        }
    }

    for (has_pane, has_inbox) in [(true, true), (true, false), (false, true)] {
        let human = Caller::default();
        let mut first = Messaging::new(Memory::default());
        first.join(&human, None, Some("g"), &Agent, 1).unwrap();
        first.join(&session("sender"), Some("sender"), Some("g"), &Agent, 2).unwrap();
        first.read(&human, None, &Agent).unwrap();
        first
            .post(&session("sender"), Some("g"), &[HUMAN.into()], "still unread", &Agent, 3)
            .unwrap();

        let mut saved = first.store().saved.clone().unwrap();
        let kept_human = saved.participants.iter_mut().find(|p| p.name == HUMAN).unwrap();
        let cursors = kept_human.cursors.clone();
        kept_human.pane = has_pane.then(|| "p1".to_string());
        kept_human.inbox = has_inbox.then(|| session("worker").inbox.unwrap());
        kept_human.woken.insert("g".into());
        let logs = first.store().logs();
        let store = Memory { kept: logs.clone(), ..Memory::default() };
        let mut second = Messaging::restore(store, saved, logs);

        let who = second.who(Some("g"), &Agent).unwrap();
        let human_member = who.iter().find(|member| member.name == HUMAN).unwrap();
        assert_eq!(human_member.pane, None);
        assert_eq!(human_member.inbox, None);
        assert_eq!(human_member.activity, None);
        assert!(second.holding().iter().all(|holding| holding.name != HUMAN));
        assert!(second.went_idle(HUMAN, &Agent, 4).0.is_empty());
        assert_eq!(second.participant(HUMAN).unwrap().cursors, cursors);

        let worker = Caller {
            directory: Some("/work/worker".into()),
            pane: has_pane.then(|| "p1".to_string()),
            inbox: has_inbox.then(|| session("worker").inbox.unwrap()),
            ..Caller::default()
        };
        let read = second.read(&worker, None, &Agent).unwrap();
        assert_ne!(read.name, HUMAN);
        assert!(read.groups.is_empty());
        assert_eq!(second.human_notice("g").count, 1);

        let posted = second
            .post(&session("sender"), Some("g"), &[HUMAN.into()], "also unread", &Agent, 5)
            .unwrap();
        assert!(
            matches!(&posted.wakes[..], [wake] if wake.name == HUMAN && wake.via == Via::Human)
        );
        let saved = second.store().saved.clone().unwrap();
        let kept_human = saved.participants.iter().find(|p| p.name == HUMAN).unwrap();
        assert_eq!(kept_human.pane, None);
        assert_eq!(kept_human.inbox, None);
        assert_eq!(kept_human.cursors, cursors);

        let mut third = Messaging::restore(Memory::default(), saved, second.store().logs());
        let read = third.read(&human, Some("g"), &Agent).unwrap();
        let messages: Vec<_> = read
            .groups
            .iter()
            .flat_map(|(_, entries)| entries)
            .filter_map(|entry| match &entry.what {
                What::Message { body, .. } => Some(body.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(messages, ["still unread", "also unread"]);
    }
}

/// The saved state may miss its last write, since a power loss can undo the rename that wrote it;
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

    let second = Messaging::restore(Memory::default(), before, first.store().logs());

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
    let logs = first.store().logs();
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

/// A delete retires the log before it saves, so a crash between the two leaves the group's policy
/// and its members' places beside a log that holds only its last number. A restore keeps neither:
/// the group stays deleted, and does not come back with the default policy.
#[test]
fn a_group_whose_log_was_removed_before_a_crash_stays_deleted() {
    let mut first = Messaging::new(Memory::default());
    first.join(&session("a"), Some("a"), Some("g"), &Everyone, 1).unwrap();
    first.join(&session("a"), Some("a"), Some("kept"), &Everyone, 2).unwrap();
    let saved = first.store().saved.clone().expect("the service saved its state");
    let mut logs = first.store().logs();
    logs.insert("g".to_string(), muster_msg::Log { after: 2, ..muster_msg::Log::default() });

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

/// A participant named after a pane it no longer holds is what an agent that named itself, and
/// was not folded into the name, left on disk. The pane means whoever is in it now.
#[test]
fn a_pane_means_whoever_holds_it_over_a_participant_named_after_it() {
    let saved: Saved = serde_json::from_value(serde_json::json!({
        "participants": [
            { "name": "p1", "cursors": { "g": 2 } },
            { "name": "builder", "pane": "p1", "cursors": { "g": 2 } },
            { "name": "a", "inbox": { "socket": "/tmp/cc-socks/a.sock", "inode": 1 },
              "cursors": { "g": 3 } },
        ],
        "groups": [],
    }))
    .unwrap();
    let joined = |seq, who: &str| muster_msg::Entry {
        seq,
        at_ms: 0,
        what: What::Joined { who: who.to_string() },
    };
    let logs = BTreeMap::from([(
        "g".to_string(),
        muster_msg::Log::from(vec![
            muster_msg::Entry { seq: 1, at_ms: 0, what: What::Created { by: "p1".to_string() } },
            joined(2, "p1"),
            joined(3, "a"),
        ]),
    )]);
    let mut service = Messaging::restore(Memory::default(), saved, logs);
    let to = ["p1".to_string()];
    let posted = service.post(&session("a"), None, &to, "who is in p1?", &Everyone, 4).unwrap();
    assert_eq!(posted.group, "a+builder");
    assert_eq!(
        posted.reached.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["builder"]
    );
}

/// A pair group deleted between two posts was made again from #1, so the second post took #4
/// again and the log lost the first. A group made again under a deleted group's name numbers on
/// from it, so no number in a group means two messages.
#[test]
fn a_group_made_again_after_a_delete_numbers_on_from_it() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("x"), &Everyone, 1).unwrap();
    service.join(&session("b"), Some("b"), Some("y"), &Everyone, 2).unwrap();
    let to = ["b".to_string()];
    let first = service.post(&session("a"), None, &to, "the first", &Everyone, 3).unwrap();
    assert_eq!((first.group.as_str(), first.seq), ("a+b", 4));
    service.group_delete(&session("a"), "a+b", &Everyone).unwrap();

    let second = service.post(&session("a"), None, &to, "the second", &Everyone, 4).unwrap();
    assert_eq!((second.group.as_str(), second.seq), ("a+b", 8));
    assert_eq!(service.log("a+b", 0).unwrap().first().map(|entry| entry.seq), Some(5));

    // And so it does after a restart, from what the store kept of the deleted group.
    service.group_delete(&session("a"), "a+b", &Everyone).unwrap();
    let store = service.store();
    let saved = store.saved.clone().expect("the service saved its state");
    let mut restored = Messaging::restore(Memory::default(), saved, store.logs());
    let third = restored.post(&session("a"), None, &to, "the third", &Everyone, 5).unwrap();
    assert_eq!(third.seq, 12);
}

/// A deleted group's name made again spelled with other letter case numbers on from it too, and
/// its old log moves to the new spelling, so a file system that ignores case does not keep the
/// new group's log under the old name.
#[test]
fn a_deleted_name_made_again_in_other_case_takes_the_old_log_over() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("Review"), &Everyone, 1).unwrap();
    service.group_delete(&session("a"), "Review", &Everyone).unwrap();
    service.join(&session("a"), Some("a"), Some("review"), &Everyone, 2).unwrap();
    assert_eq!(service.store().renamed, [("Review".to_string(), "review".to_string())]);
    assert_eq!(service.log("review", 0).unwrap().first().map(|entry| entry.seq), Some(3));
}

/// The move comes before the new group's first entry, so a first entry the disk refuses leaves
/// the number kept there: a restart still numbers the group on from the deleted one.
#[test]
fn a_name_made_again_in_other_case_keeps_its_number_through_a_failed_first_entry() {
    let mut service = Messaging::new(Memory::default());
    service.join(&session("a"), Some("a"), Some("Review"), &Everyone, 1).unwrap();
    service.group_delete(&session("a"), "Review", &Everyone).unwrap();
    service.store_mut().failing_appends = true;
    let refused = service.join(&session("a"), Some("a"), Some("review"), &Everyone, 2);
    assert!(matches!(refused, Err(Refusal::Store { .. })), "{refused:?}");

    let store = service.store();
    let saved = store.saved.clone().expect("the service saved its state");
    let mut restored = Messaging::restore(Memory::default(), saved, store.logs());
    restored.join(&session("a"), Some("a"), Some("review"), &Everyone, 3).unwrap();
    assert_eq!(restored.log("review", 0).unwrap().first().map(|entry| entry.seq), Some(3));
}
