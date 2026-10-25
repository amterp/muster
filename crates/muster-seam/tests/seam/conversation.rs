//! A group's conversation shown in a window, through a real daemon
//! (mip/0009-group-conversation.md, sections 1-4 and 7): the window shows no panes while it is
//! up, it reads the group only in the window in front, it follows the group's log, choosing where
//! to look leaves it, and the message box keeps a draft and posts it with each of a send's
//! outcomes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::group_messages::{
    entries, group_of_three, in_pane, log, msg, named, post, posted, the_human, unread,
};
use muster::proto::{
    ConversationChanged, ConversationMembersChanged, CreateTab, Event, FocusPane,
    LeaveConversation, LoadEarlier, MessageBoxChanged, OpenWindow, PauseGroup,
    PickConversationAnswer, PostFromConversation, Quitting, Request, Response, SaveDraft,
    ShowConversation, Startup, ViewChanged, WindowFocus, conversation_row, event, post_outcome,
    request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{Answer, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::request::Service;
use muster_harness::requests::{create, expect, in_new_tab, make, snapshot, until_text};
use muster_harness::{Control, Daemon, Relay, until};
use prost::Message;

#[test]
fn a_pane_that_finishes_under_a_conversation_is_announced_and_keeps_its_done() {
    let open = Open::detecting();
    assert_ok(&answer(focus(true)));
    show("window-1");
    until(
        "the view to say the window shows the conversation",
        || view("window-1").conversation_group == "g",
        || format!("the view is {:?}", view("window-1")),
    );

    open.daemon.set_agent_state("p1", daemon_proto::AgentState::Working);
    until_state("p1", "working");
    open.daemon.set_agent_state("p1", daemon_proto::AgentState::Idle);
    until_state("p1", "done");

    assert!(answer(leave()).payload.is_some());
    until_state("p1", "idle");
    let back = view("window-1");
    assert_eq!(
        (back.conversation_group.as_str(), back.keyboard_pane_id.as_str()),
        ("", "p1"),
        "leaving puts the panes back with the keyboard where it was"
    );
}

#[test]
fn showing_a_conversation_reads_the_group_and_follows_it() {
    let mut open = Open::built();
    post(&mut open.control, "a", "b", "between agents");
    post(&mut open.control, "b", "a", "still between agents");
    let asked = post(&mut open.control, "a", "@human", "ok to merge?");
    assert_eq!(unread(&mut open.control), 1);

    assert_ok(&answer(focus(true)));
    show("window-1");
    until_read(&open.daemon, "the human's place to move to the end once the conversation is read");
    let rows = until_rows(|rows| rows.iter().any(|row| is_message(row, asked)));
    let kinds: Vec<&str> = rows.iter().map(kind).collect();
    assert_eq!(
        kinds,
        ["divider", "changes", "fold", "unread", "message"],
        "two messages between agents fold, and the unread line is before the question"
    );

    let next = post(&mut open.control, "b", "@human", "and the build is green");
    until_rows(|rows| rows.iter().any(|row| is_message(row, next)));
    until_read(&open.daemon, "a message arriving while the conversation is read to be read");
}

#[test]
fn a_message_arriving_while_the_conversation_is_read_is_read_once() {
    let reads = Arc::new(AtomicUsize::new(0));
    let (mut open, relay) = Open::relayed(|daemon| {
        let reads = Arc::clone(&reads);
        daemon.withholding_answers_where(move |request| {
            if is_a_read_by_the_human(request) {
                reads.fetch_add(1, Ordering::SeqCst);
            }
            false
        })
    });
    assert_ok(&answer(focus(true)));
    show("window-1");
    until_loaded("window-1");
    let before = reads.load(Ordering::SeqCst);

    let asked = post(&mut open.control, "a", "@human", "ok to merge?");
    until_rows(|rows| rows.iter().any(|row| is_message(row, asked)));
    until_read(&open.daemon, "the message to be read while the conversation is read");
    // Long enough for a second read, which nothing would wait for, to reach the daemon.
    std::thread::sleep(Duration::from_millis(500));

    assert_eq!(reads.load(Ordering::SeqCst) - before, 1, "read once, not twice");
    drop(relay);
}

#[test]
fn a_second_window_on_the_group_reads_nothing_until_it_comes_to_the_front() {
    let mut open = Open::built();
    open_second_window(&open.daemon);
    assert_ok(&answer(focus(true)));
    post(&mut open.control, "a", "@human", "ok to merge?");

    show("window-2");
    until(
        "the second window's conversation to load",
        || conversation("window-2").rows.len() > 1,
        || format!("window-2 was told {:?}", conversation("window-2")),
    );
    assert_eq!(unread(&mut open.control), 1, "a window that is not in front reads nothing");

    assert_ok(&answer_in("window-2", focus(true)));
    until_read(&open.daemon, "the group to be read once its window comes to the front");
}

#[test]
fn two_windows_on_one_group_share_one_fetch() {
    static FETCHES: AtomicUsize = AtomicUsize::new(0);
    let _turn = fresh();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let relay = daemon.withholding_answers_where(|request| {
        if is_a_first_page(request) {
            FETCHES.fetch_add(1, Ordering::SeqCst);
        }
        false
    });
    start_window(&relay.muster_config(), &daemon.root().join("windows/window-1.toml"));
    open_second_window(&daemon);
    group_of_three(&mut control);
    post(&mut control, "a", "@human", "ok to merge?");

    show("window-1");
    show("window-2");
    for window in ["window-1", "window-2"] {
        until(
            "both windows' conversations to load",
            || conversation(window).rows.len() > 1,
            || format!("{window} was told {:?}", conversation(window)),
        );
    }
    assert_eq!(FETCHES.load(Ordering::SeqCst), 1, "the second window used the first one's copy");
    drop(relay);
}

#[test]
fn leaving_and_showing_a_group_again_at_once_leaves_one_follow() {
    let follows = Arc::new(AtomicUsize::new(0));
    let (mut open, relay) = Open::relayed(|daemon| {
        let follows = Arc::clone(&follows);
        daemon.withholding_answers_where(move |request| {
            if is_a_follow(request) {
                follows.fetch_add(1, Ordering::SeqCst);
            }
            false
        })
    });
    show("window-1");
    until_loaded("window-1");
    until_follows(&follows, 1);

    // Within one of the follow's waits, so the thread following the first log is still in one.
    answer(leave());
    show("window-1");
    until_loaded("window-1");
    until_follows(&follows, 2);
    // Longer than the follow's wait, so a thread still following the old log has looked again.
    std::thread::sleep(Duration::from_millis(600));
    let seq = post(&mut open.control, "a", "b", "after showing it again");
    until_rows(|rows| rows.iter().any(|row| is_message(row, seq)));
    std::thread::sleep(Duration::from_millis(600));

    assert_eq!(
        follows.load(Ordering::SeqCst),
        3,
        "one follow for each log, and one more once the post arrived: a second thread follows too"
    );
    drop(relay);
}

#[test]
fn a_group_the_human_is_not_in_is_refused_naming_those_they_are_in() {
    let first_pages = Arc::new(AtomicUsize::new(0));
    let (_open, relay) = Open::relayed(|daemon| {
        let first_pages = Arc::clone(&first_pages);
        daemon.withholding_answers_where(move |request| {
            if is_a_first_page(request) {
                first_pages.fetch_add(1, Ordering::SeqCst);
            }
            false
        })
    });
    show("window-1");
    until_loaded("window-1");
    answer(leave());

    let refused = answer_in(
        "window-1",
        request::Payload::ShowConversation(ShowConversation {
            daemon_id: String::new(),
            group: "gg".to_string(),
        }),
    );
    let Some(response::Payload::Failure(failure)) = &refused.payload else {
        panic!("showing a group nobody made answered {refused:?}");
    };
    assert!(
        failure.reason.contains("no group called gg") && failure.reason.contains("are g"),
        "and says which groups there are: {}",
        failure.reason
    );
    assert!(view("window-1").conversation_group.is_empty(), "the window shows its panes");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(first_pages.load(Ordering::SeqCst), 1, "and nothing fetches a log for it");
    drop(relay);
}

#[test]
fn choosing_a_pane_that_is_not_there_leaves_the_human_reading() {
    let _open = Open::built();
    show("window-1");
    until_shown("window-1", "g");

    let refused = answer(request::Payload::FocusPane(FocusPane {
        daemon_id: "local".to_string(),
        pane_id: "p404".to_string(),
    }));
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "going to a pane nobody made answered {refused:?}"
    );
    // A leave publishes at once; give one the chance to land before saying none did.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(view("window-1").conversation_group, "g", "the window still shows the group");
}

#[test]
fn choosing_where_to_look_leaves_the_conversation_and_a_tab_an_agent_makes_does_not() {
    let _open = Open::built();
    show("window-1");
    until_shown("window-1", "g");

    let socket_tab = Request::new(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    }));
    let made =
        Response::decode(muster::dispatch_from_socket(&socket_tab.encode_to_vec()).as_slice())
            .expect("a response this build knows");
    assert_ok(&made);
    assert_eq!(view("window-1").conversation_group, "g", "an agent's new tab leaves it be");

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until_shown("window-1", "");

    show("window-1");
    until_shown("window-1", "g");
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id: "local".to_string(),
        pane_id: "p1".to_string(),
    })));
    until_shown("window-1", "");
    assert_eq!(view("window-1").keyboard_pane_id, "p1", "and the keyboard is on what was chosen");
}

#[test]
fn typing_in_the_message_box_is_answered_with_the_draft_and_sends_no_event() {
    let mut open = Open::built();
    post(&mut open.control, "a", "@human", "ok to merge?");
    show("window-1");
    until(
        "the members and the opening draft",
        || {
            conversation("window-1").header.is_some_and(|header| header.members_known)
                && message_box("window-1").draft.is_some()
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
    let opened = message_box("window-1").draft.unwrap_or_default();
    let to: Vec<&str> = opened.to.iter().map(|to| to.name.as_str()).collect();
    assert_eq!(to, ["a"], "it opens addressed to whoever last wrote to the human");

    let told = told_count();
    let typed = answer(save_draft("@b ", 3, &["a"]));
    let Some(response::Payload::Draft(draft)) = typed.payload else {
        panic!("a draft was answered with {typed:?}");
    };
    let to: Vec<&str> = draft.to.iter().map(|to| to.name.as_str()).collect();
    assert_eq!((draft.text.as_str(), to.as_slice(), draft.cursor), ("", &["b"][..], 0));
    assert_eq!(told_count(), told, "typing sent the shell no event");
}

#[test]
fn choosing_a_completion_at_the_start_addresses_the_member() {
    let _open = Open::built();
    show("window-1");
    until_loaded("window-1");
    let typed = answer(save_draft("@", 1, &[]));
    let Some(response::Payload::Draft(typed)) = typed.payload else {
        panic!("typing answered {typed:?}");
    };
    let offered: Vec<&str> = typed.completions.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(offered, ["a", "b"], "both agents are offered");

    let chosen = answer(request::Payload::SaveDraft(SaveDraft {
        complete: "b".to_string(),
        ..draft_edit("@", 1, &[])
    }));
    let Some(response::Payload::Draft(draft)) = chosen.payload else {
        panic!("choosing a completion answered {chosen:?}");
    };
    let to: Vec<&str> = draft.to.iter().map(|to| to.name.as_str()).collect();
    assert_eq!((draft.text.as_str(), to.as_slice(), draft.cursor), ("", &["b"][..], 0));
}

#[test]
fn a_post_from_the_message_box_is_kept_and_its_draft_clears() {
    let mut open = Open::built();
    show("window-1");
    until_loaded("window-1");
    answer(save_draft("ship it", 7, &["a"]));

    let posted = answer(post_draft());
    let Some(response::Payload::Ok(ok)) = &posted.payload else {
        panic!("the post answered {posted:?}");
    };
    assert_eq!(ok.note, "nobody live heard it", "a joined with no inbox, so nothing woke it");
    let (author, to, body) = entries(&mut open.control).pop().expect("the post is in the log");
    assert_eq!(
        (author.as_str(), to.as_slice(), body.as_str()),
        ("@human", &["a".to_string()][..], "ship it")
    );
    until(
        "the message box to say it was posted and clear",
        || {
            let told = message_box("window-1");
            told.sent.is_some_and(|sent| sent.state() == post_outcome::State::Posted)
                && told.draft.is_some_and(|draft| draft.text.is_empty())
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
}

/// A question's answers are drawn under it; picking one fills the message box, addressed to the
/// question's author, and the post is the answer with the note under it, carrying `answering`,
/// after which the question says the human answered it and offers nothing more.
#[test]
fn a_questions_answer_is_picked_into_the_message_box_and_posted_with_it() {
    let mut open = Open::built();
    let question = ask(&mut open.control, "run the contract tier?", &["Run it now", "Skip it"]);
    show("window-1");
    until_loaded("window-1");
    until(
        "the question's row to offer its answers",
        || {
            row("window-1", question)
                .is_some_and(|row| row.answers == ["Run it now", "Skip it"] && row.answerable)
        },
        || format!("the row is {:?}", row("window-1", question)),
    );

    let Some(response::Payload::Draft(picked)) = answer(pick(question, Some(1))).payload else {
        panic!("a pick answered with no draft");
    };
    let to: Vec<&str> = picked.to.iter().map(|to| to.name.as_str()).collect();
    assert_eq!(to, ["a"], "a pick addresses whoever asked");
    assert_eq!((picked.picked_question, picked.picked_answer.as_str()), (question, "Skip it"));
    assert!(picked.sendable, "an answer picked with no words is a whole reply");
    answer(save_draft("after lunch", 11, &["a"]));

    let posted = answer(post_draft());
    assert!(
        matches!(posted.payload, Some(response::Payload::Ok(_))),
        "the post answered {posted:?}"
    );
    let (author, to, body) = entries(&mut open.control).pop().expect("the post is in the log");
    assert_eq!(
        (author.as_str(), to.as_slice(), body.as_str()),
        ("@human", &["a".to_string()][..], "Answer: Skip it\n\nafter lunch")
    );
    let answering = match log(&mut open.control).pop().and_then(|entry| entry.what) {
        Some(What::Message(message)) => message.answering,
        _ => None,
    };
    assert_eq!(
        answering,
        Some(msg_request::Answering { question, answer: "Skip it".to_string() }),
        "named as the post's field, so every view can mark the question answered"
    );
    until(
        "the question's row to say the human answered it",
        || {
            row("window-1", question).is_some_and(|row| {
                (row.answered_by.as_str(), row.answered_with.as_str()) == ("you", "Skip it")
                    && !row.answerable
            })
        },
        || format!("the row is {:?}", row("window-1", question)),
    );
}

/// The draft keeps its pick whole, so a relaunch brings the answer back with the words.
#[test]
fn a_picked_answer_survives_a_relaunch() {
    let mut open = Open::built();
    let question = ask(&mut open.control, "ok to merge?", &["Merge", "Hold off"]);
    show("window-1");
    until_loaded("window-1");
    until(
        "the question's row",
        || row("window-1", question).is_some_and(|row| row.answerable),
        || format!("the row is {:?}", row("window-1", question)),
    );
    answer(pick(question, Some(0)));
    let arrangement = open.daemon.root().join("windows/window-1.toml");
    until(
        "the pick to be written down",
        || std::fs::read_to_string(&arrangement).is_ok_and(|text| text.contains("Merge")),
        || std::fs::read_to_string(&arrangement).unwrap_or_default(),
    );

    open.turn.relaunch();
    muster::ffi::muster_set_event_callback(Some(note));
    forget_events();
    start_window(&open.daemon.muster_config(), &arrangement);
    show("window-1");
    until(
        "the pick to come back with the window",
        || {
            message_box("window-1").draft.is_some_and(|draft| {
                (draft.picked_question, draft.picked_answer.as_str()) == (question, "Merge")
            })
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
}

#[test]
fn a_post_the_daemon_refuses_keeps_its_draft() {
    let mut open = Open::built();
    show("window-1");
    until_loaded("window-1");
    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut open.control, msg(&named("a"), leave), daemon_proto::Outcome::Done);
    answer(save_draft("yes", 3, &["a"]));

    let refused = answer(post_draft());
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "a post to an author who left answered {refused:?}"
    );
    until(
        "the message box to say it was refused and keep its words",
        || {
            let told = message_box("window-1");
            told.sent.is_some_and(|sent| sent.state() == post_outcome::State::Refused)
                && told.draft.is_some_and(|draft| draft.text == "yes")
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
}

/// With an answer picked, the post's body is composed from the pick and the note. A refusal
/// keeps the note the human wrote, not that body, so sending again does not write the answer
/// twice.
#[test]
fn a_refused_post_with_an_answer_picked_keeps_the_words_typed() {
    let mut open = Open::built();
    let question = ask(&mut open.control, "run the contract tier?", &["Run it now", "Skip it"]);
    show("window-1");
    until_loaded("window-1");
    until(
        "the question's row to offer its answers",
        || row("window-1", question).is_some_and(|row| row.answerable),
        || format!("the row is {:?}", row("window-1", question)),
    );
    answer(pick(question, Some(1)));
    answer(save_draft("after lunch", 11, &["a"]));
    let leave = Asked::Leave(msg_request::Leave { group: Some("g".to_string()) });
    expect(&mut open.control, msg(&named("a"), leave), daemon_proto::Outcome::Done);

    let refused = answer(post_draft());
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "a post to an author who left answered {refused:?}"
    );
    until_sent("window-1", post_outcome::State::Refused);
    let sent = message_box("window-1").sent.expect("the box says where its post stands");
    assert_eq!(sent.body, "after lunch");
}

#[test]
fn a_post_whose_answer_never_came_is_found_in_the_log_and_not_sent_twice() {
    // Takes the client's whole patience, ten seconds, because an answer the window has stopped
    // waiting for is the case under test and nothing shortens that wait from outside.
    let _turn = fresh();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let relay = daemon.withholding_answers_where(is_a_post_by_the_human);
    start_window(&relay.muster_config(), &daemon.root().join("windows/window-1.toml"));
    group_of_three(&mut control);
    until_listed("g");
    show("window-1");
    until_loaded("window-1");
    answer(save_draft("yes", 3, &["a"]));

    let posted = answer(post_draft());
    let Some(response::Payload::Ok(ok)) = &posted.payload else {
        panic!("a post kept but never answered, then found in the log, answered {posted:?}");
    };
    assert!(ok.note.contains("never answered"), "and says so: {}", ok.note);
    let sent = entries(&mut control)
        .into_iter()
        .filter(|(author, _, body)| author == "@human" && body == "yes")
        .count();
    assert_eq!(sent, 1, "posted once and not sent again");
    until(
        "the draft to clear",
        || message_box("window-1").draft.is_some_and(|draft| draft.text.is_empty()),
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
    drop(relay);
}

#[test]
fn words_written_while_a_post_is_on_its_way_stay_once_it_is_kept() {
    let (_open, relay) = Open::relayed(|daemon| {
        daemon.delaying_answers_where(is_a_post_by_the_human, Duration::from_millis(1500))
    });
    show("window-1");
    until_loaded("window-1");
    answer(save_draft("ship it", 7, &["a"]));

    let posting = std::thread::spawn(|| answer(post_draft()));
    until_sent("window-1", post_outcome::State::Pending);
    answer(save_draft("and the docs", 12, &["a"]));
    assert_ok(&posting.join().expect("the post's thread"));

    until(
        "the post to be kept, with what was written since still in the box",
        || {
            let told = message_box("window-1");
            told.sent.is_some_and(|sent| sent.state() == post_outcome::State::Posted)
                && told.draft.is_some_and(|draft| draft.text == "and the docs")
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
    drop(relay);
}

#[test]
fn words_written_while_a_post_went_unanswered_stay_once_the_log_settles_it() {
    // Up to twenty seconds: the client's whole patience for the post, and again for the read
    // that looks for it in the log, both withheld so that only the follow can settle it.
    let reads_withheld = Arc::new(AtomicBool::new(false));
    let (mut open, relay) = Open::relayed(|daemon| {
        let reads_withheld = Arc::clone(&reads_withheld);
        daemon.withholding_answers_where(move |request| {
            is_a_post_by_the_human(request)
                || (reads_withheld.load(Ordering::SeqCst) && is_a_log_read_by_the_human(request))
        })
    });
    show("window-1");
    until_loaded("window-1");
    reads_withheld.store(true, Ordering::SeqCst);
    answer(save_draft("yes", 3, &["a"]));

    let posting = std::thread::spawn(|| answer(post_draft()));
    until_sent("window-1", post_outcome::State::Pending);
    answer(save_draft("no, wait", 8, &["a"]));
    let unanswered = posting.join().expect("the post's thread");
    assert!(
        !matches!(unanswered.payload, Some(response::Payload::Ok(_))),
        "a post with no answer and no log to find it in answered {unanswered:?}"
    );
    // Something new in the log has the conversation look for the post again.
    post(&mut open.control, "b", "@human", "and another thing");

    until(
        "the post to settle as kept, with what was written since still in the box",
        || {
            let told = message_box("window-1");
            told.sent.is_some_and(|sent| sent.state() == post_outcome::State::Posted)
                && told.draft.is_some_and(|draft| draft.text == "no, wait")
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
    drop(relay);
}

#[test]
fn an_unanswered_post_whose_entry_came_while_it_was_pending_settles() {
    // Up to twenty seconds, for the reason the test above gives. Nothing is posted afterwards, so
    // only the entry the follow brought while the post was pending can settle it.
    let reads_withheld = Arc::new(AtomicBool::new(false));
    let (_open, relay) = Open::relayed(|daemon| {
        let reads_withheld = Arc::clone(&reads_withheld);
        daemon.withholding_answers_where(move |request| {
            is_a_post_by_the_human(request)
                || (reads_withheld.load(Ordering::SeqCst) && is_a_log_read_by_the_human(request))
        })
    });
    show("window-1");
    until_loaded("window-1");
    reads_withheld.store(true, Ordering::SeqCst);
    answer(save_draft("yes", 3, &["a"]));

    let unanswered = answer(post_draft());
    assert!(
        !matches!(unanswered.payload, Some(response::Payload::Ok(_))),
        "a post with no answer and no log to find it in answered {unanswered:?}"
    );

    until(
        "the post to settle from the entry the follow already brought",
        || {
            let told = message_box("window-1");
            told.sent.is_some_and(|sent| sent.state() == post_outcome::State::Posted)
                && told.draft.is_some_and(|draft| draft.text.is_empty())
        },
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
    drop(relay);
}

#[test]
fn two_windows_keep_two_drafts_and_a_draft_survives_a_relaunch() {
    let open = Open::built();
    open_second_window(&open.daemon);
    show("window-1");
    show("window-2");
    for window in ["window-1", "window-2"] {
        until_loaded(window);
    }
    answer(save_draft("from one", 8, &[]));
    answer_in("window-2", save_draft("from two", 8, &[]));
    let arrangement = open.daemon.root().join("windows/window-1.toml");
    until(
        "the first window's draft to be written down",
        || std::fs::read_to_string(&arrangement).is_ok_and(|text| text.contains("from one")),
        || std::fs::read_to_string(&arrangement).unwrap_or_default(),
    );
    assert!(
        !std::fs::read_to_string(&arrangement).unwrap_or_default().contains("from two"),
        "each window's message box is its own"
    );

    open.turn.relaunch();
    muster::ffi::muster_set_event_callback(Some(note));
    forget_events();
    start_window(&open.daemon.muster_config(), &arrangement);
    show("window-1");
    until(
        "the draft to come back with the window",
        || message_box("window-1").draft.is_some_and(|draft| draft.text == "from one"),
        || format!("window-1 was told {:?}", message_box("window-1")),
    );
}

#[test]
fn a_draft_written_just_before_quitting_is_kept() {
    let open = Open::built();
    show("window-1");
    until_loaded("window-1");
    let arrangement = open.daemon.root().join("windows/window-1.toml");

    answer(save_draft("last words", 10, &[]));
    assert_ok(&answer(request::Payload::Quitting(Quitting { close_sessions: false })));

    assert!(
        std::fs::read_to_string(&arrangement).is_ok_and(|text| text.contains("last words")),
        "the draft was not written down before the quit, so it is gone on the next launch: {}",
        std::fs::read_to_string(&arrangement).unwrap_or_default()
    );
}

#[test]
fn a_long_group_is_paged_newest_first() {
    let mut open = Open::built();
    let first = post(&mut open.control, "a", "@human", "the first");
    for n in 0..205 {
        post(&mut open.control, "a", "@human", &format!("message {n}"));
    }
    show("window-1");
    until_loaded("window-1");
    let told = conversation("window-1");
    assert!(told.earlier, "the first page leaves out the oldest entries");
    assert!(!told.rows.iter().any(|row| is_message(row, first)));

    assert_ok(&answer(request::Payload::LoadEarlier(LoadEarlier {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
    })));
    until(
        "the page before to arrive",
        || conversation("window-1").rows.iter().any(|row| is_message(row, first)),
        || format!("the earliest row is {:?}", conversation("window-1").rows.get(1)),
    );
}

#[test]
fn the_members_column_says_why_an_agent_has_not_read_and_where_it_is() {
    let mut open = Open::detecting();
    let join = Asked::Join(msg_request::Join {
        group: Some("g".to_string()),
        ..msg_request::Join::default()
    });
    expect(&mut open.control, msg(&in_pane("p1"), join), daemon_proto::Outcome::Done);
    let asked = post(&mut open.control, "a", "p1", "can you look?");
    show("window-1");
    let agent = || members("window-1").members.into_iter().find(|member| member.name == "p1");
    until(
        "the members column to say the agent in p1 has not read",
        || agent().is_some_and(|member| member.unread_first == asked),
        || format!("the members are {:?}", members("window-1")),
    );
    let agent = agent().unwrap_or_default();
    assert!(!agent.unread_why.is_empty(), "the daemon's reason comes with it: {agent:?}");
    assert_eq!((agent.daemon_id.as_str(), agent.pane_id.as_str()), ("local", "p1"));
    assert_ne!(agent.shown, "p1", "an agent is shown by its callsign, not its pane's id");
}

#[test]
fn what_the_daemon_wakes_the_human_for_is_what_the_thread_leaves_unfolded() {
    // The thread decides which unaddressed messages woke the human from the group's policy, as
    // the daemon does when it tells the window what waits; a fold that disagreed would hide a
    // message the daemon raised for the human, and nothing would say so.
    let mut open = Open::built();
    let mut ring = std::collections::HashMap::new();
    ring.insert("director".to_string(), names(&["@human"]));
    ring.insert("*".to_string(), names(&["director"]));
    let policy = msg_request::Policy {
        ring,
        allow: std::collections::HashMap::from([("*".to_string(), names(&["*"]))]),
        membership: vec!["*".to_string()],
        ..msg_request::Policy::default()
    };
    let made =
        Asked::GroupNew(msg_request::GroupNew { group: "crew".to_string(), policy: Some(policy) });
    expect(&mut open.control, msg(&named("director"), made), daemon_proto::Outcome::Done);
    for (caller, name) in [(the_human(), "@human"), (named("a"), "a"), (named("b"), "b")] {
        let join = Asked::Join(msg_request::Join {
            name: Some(name.to_string()),
            group: Some("crew".to_string()),
            pull: false,
        });
        expect(&mut open.control, msg(&caller, join), daemon_proto::Outcome::Done);
    }
    // Every run of agents' messages between the director's is two or more long, so each folds
    // unless the thread thinks it woke the human.
    for author in ["director", "a", "b", "director", "a", "b", "a", "director"] {
        let read = Asked::Read(msg_request::Read { group: Some("crew".to_string()) });
        open.control.ask(msg(&named(author), read));
        let asked = Asked::Post(msg_request::Post {
            group: Some("crew".to_string()),
            body: format!("from {author}"),
            ..Default::default()
        });
        expect(&mut open.control, msg(&named(author), asked), daemon_proto::Outcome::Done);
    }
    let notice = snapshot(&mut open.control)
        .human
        .into_iter()
        .find(|notice| notice.group == "crew")
        .expect("the daemon says what waits for the human in crew");
    until_listed("crew");

    let shown = answer_in(
        "window-1",
        request::Payload::ShowConversation(ShowConversation {
            daemon_id: String::new(),
            group: "crew".to_string(),
        }),
    );
    assert!(matches!(shown.payload, Some(response::Payload::Opened(_))), "{shown:?}");
    let rows = until_rows(|rows| {
        rows.iter().filter(|row| kind(row) == "message").count() as u64 >= notice.count
            && rows.iter().any(|row| kind(row) == "fold")
    });
    let unfolded: Vec<u64> = rows
        .iter()
        .filter_map(|row| match &row.kind {
            Some(conversation_row::Kind::Message(message)) => Some(message.seq),
            _ => None,
        })
        .collect();

    assert_eq!(
        (unfolded.len() as u64, unfolded.first().copied(), unfolded.last().copied()),
        (notice.count, Some(notice.first), Some(notice.last)),
        "the daemon woke the human for {notice:?}, and the thread left {unfolded:?} unfolded"
    );
}

#[test]
fn pausing_from_the_header_marks_the_group_paused() {
    let _open = Open::built();
    show("window-1");
    until_loaded("window-1");
    assert_ok(&answer(request::Payload::PauseGroup(PauseGroup {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        paused: true,
    })));
    until(
        "the header to say the group is paused",
        || conversation("window-1").header.is_some_and(|header| header.paused),
        || format!("the header is {:?}", conversation("window-1").header),
    );
}

/// A window on a daemon holding `g`, with the human, `a` and `b` in it.
struct Open {
    turn: muster::testing::Turn,
    daemon: Daemon,
    control: Control,
}

impl Open {
    fn built() -> Open {
        let turn = fresh();
        let daemon = Daemon::start_built();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        until_text(&mut control, "p1", "$");
        start_window(&daemon.muster_config(), &daemon.root().join("windows/window-1.toml"));
        group_of_three(&mut control);
        until_listed("g");
        Open { turn, daemon, control }
    }

    /// The same, with the window reaching the daemon through a relay made from it.
    fn relayed(relay: impl FnOnce(&Daemon) -> Relay) -> (Open, Relay) {
        let turn = fresh();
        let daemon = Daemon::start_built();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        until_text(&mut control, "p1", "$");
        let relay = relay(&daemon);
        start_window(&relay.muster_config(), &daemon.root().join("windows/window-1.toml"));
        group_of_three(&mut control);
        until_listed("g");
        (Open { turn, daemon, control }, relay)
    }

    /// With an agent in its pane whose state a test can set.
    fn detecting() -> Open {
        let turn = fresh();
        let daemon = Daemon::start_detecting();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        daemon.run_agent("p1");
        start_window(&daemon.muster_config(), &daemon.root().join("windows/window-1.toml"));
        group_of_three(&mut control);
        until_listed("g");
        Open { turn, daemon, control }
    }
}

fn fresh() -> muster::testing::Turn {
    let turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    forget_events();
    turn
}

/// Starts the app and opens its first window, remembering its arrangement at `arrangement` and
/// which window holds each tab beside it, so windows are named as they are in the app.
fn start_window(config: &std::path::Path, arrangement: &std::path::Path) {
    let root = arrangement.parent().and_then(std::path::Path::parent).expect("windows/ in a root");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement.to_string_lossy().into_owned(),
        tab_holders_path: root.join("holding/tabs.toml").to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
}

fn open_second_window(daemon: &Daemon) {
    let arrangement: PathBuf = daemon.root().join("windows/window-2.toml");
    let opened = answer(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement.to_string_lossy().into_owned(),
        ..OpenWindow::default()
    }));
    match opened.payload {
        Some(response::Payload::Opened(opened)) => assert_eq!(opened.window, "window-2"),
        other => panic!("opening a second window answered {other:?}"),
    }
}

fn show(window: &str) {
    let shown = answer_in(
        window,
        request::Payload::ShowConversation(ShowConversation {
            daemon_id: String::new(),
            group: "g".to_string(),
        }),
    );
    match shown.payload {
        Some(response::Payload::Opened(opened)) => assert_eq!(opened.window, window),
        other => panic!("showing the conversation answered {other:?}"),
    }
}

fn leave() -> request::Payload {
    request::Payload::LeaveConversation(LeaveConversation {})
}

fn focus(focused: bool) -> request::Payload {
    request::Payload::WindowFocus(WindowFocus { focused })
}

fn save_draft(text: &str, cursor: u32, to: &[&str]) -> request::Payload {
    request::Payload::SaveDraft(draft_edit(text, cursor, to))
}

fn draft_edit(text: &str, cursor: u32, to: &[&str]) -> SaveDraft {
    SaveDraft {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        text: text.to_string(),
        cursor,
        to: to.iter().map(ToString::to_string).collect(),
        ..SaveDraft::default()
    }
}

fn pick(question: u64, answer: Option<u32>) -> request::Payload {
    request::Payload::PickConversationAnswer(PickConversationAnswer {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
        question,
        answer,
    })
}

/// Asks the human a question as `a`, offering `answers`, and returns its number.
fn ask(control: &mut Control, body: &str, answers: &[&str]) -> u64 {
    match posted(control, &named("a"), "@human", body, answers).answer {
        Some(Answer::Posted(posted)) => posted.seq,
        other => panic!("a question answered with {other:?}"),
    }
}

/// Message `seq`'s row in the conversation `window` was last told of.
fn row(window: &str, seq: u64) -> Option<muster::proto::ConversationMessage> {
    conversation(window).rows.into_iter().find_map(|row| match row.kind {
        Some(conversation_row::Kind::Message(message)) if message.seq == seq => Some(*message),
        _ => None,
    })
}

fn post_draft() -> request::Payload {
    request::Payload::PostFromConversation(PostFromConversation {
        daemon_id: "local".to_string(),
        group: "g".to_string(),
    })
}

fn until_sent(window: &str, state: post_outcome::State) {
    until(
        "the message box to say where its post stands",
        || message_box(window).sent.is_some_and(|sent| sent.state() == state),
        || format!("{window} was told {:?}", message_box(window)),
    );
}

/// Waits for the window to hear that the human is in `group`, which a conversation shows only
/// once it has.
fn until_listed(group: &str) {
    until(
        "the window to hear the human is in the group",
        || {
            events().iter().any(|told| {
                matches!(&told.payload, Some(event::Payload::GroupsChanged(changed))
                    if changed.groups.iter().any(|listed| listed.group == group))
            })
        },
        || "no groups event named it".to_string(),
    );
}

fn until_shown(window: &str, group: &str) {
    until(
        "the view to say which conversation the window shows",
        || view(window).conversation_group == group,
        || format!("the view is {:?}", view(window)),
    );
}

fn until_loaded(window: &str) {
    until(
        "the conversation's first page and members",
        || {
            let told = conversation(window);
            !told.loading
                && !told.daemon_id.is_empty()
                && told.header.is_some_and(|header| header.members_known)
        },
        || format!("{window} was told {:?}", conversation(window)),
    );
}

fn until_rows(
    wanted: impl Fn(&[muster::proto::ConversationRow]) -> bool,
) -> Vec<muster::proto::ConversationRow> {
    until(
        "the thread to hold what the test posted",
        || wanted(&conversation("window-1").rows),
        || format!("the rows are {:?}", conversation("window-1").rows),
    );
    conversation("window-1").rows
}

fn until_state(pane: &str, state: &str) {
    until(
        &format!("the window to say {pane} is {state}"),
        || latest_state(pane).as_deref() == Some(state),
        || format!("the window last said {:?}", latest_state(pane)),
    );
}

fn is_message(row: &muster::proto::ConversationRow, seq: u64) -> bool {
    matches!(&row.kind, Some(conversation_row::Kind::Message(message)) if message.seq == seq)
}

fn kind(row: &muster::proto::ConversationRow) -> &'static str {
    match row.kind {
        Some(conversation_row::Kind::Divider(_)) => "divider",
        Some(conversation_row::Kind::Changes(_)) => "changes",
        Some(conversation_row::Kind::Fold(_)) => "fold",
        Some(conversation_row::Kind::Message(_)) => "message",
        Some(conversation_row::Kind::Unread(_)) => "unread",
        None => "none",
    }
}

/// A post's first page fetched as the human: what a conversation sends once for each group it
/// shows.
fn is_a_first_page(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest {
            caller: Some(caller),
            request: Some(Asked::Log(log)),
        })) if caller.as_name.is_none() && log.newest.is_some() && log.before.is_none()
    )
}

/// A read of the log as the human that is neither a page nor a follow: how a post that was never
/// answered is looked for.
fn is_a_log_read_by_the_human(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest {
            caller: Some(caller),
            request: Some(Asked::Log(log)),
        })) if caller.as_name.is_none() && !log.follow && log.newest.is_none()
    )
}

fn is_a_follow(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest { request: Some(Asked::Log(log)), .. }))
            if log.follow
    )
}

fn until_follows(follows: &AtomicUsize, count: usize) {
    until(
        "the conversation to follow the group's log",
        || follows.load(Ordering::SeqCst) >= count,
        || format!("the daemon was asked to follow {} times", follows.load(Ordering::SeqCst)),
    );
}

fn is_a_read_by_the_human(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest { caller: Some(caller), request: Some(Asked::Read(_)) }))
            if caller.as_name.is_none()
    )
}

fn is_a_post_by_the_human(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Msg(daemon_proto::MsgRequest { caller: Some(caller), request: Some(Asked::Post(_)) }))
            if caller.as_name.is_none()
    )
}

/// Waits for the daemon to say nothing waits for the human in `g` any more.
fn until_read(daemon: &Daemon, what: &str) {
    until(
        what,
        || unread(&mut daemon.connect()) == 0,
        || format!("{} still wait", unread(&mut daemon.connect())),
    );
}

fn names(names: &[&str]) -> msg_request::Names {
    msg_request::Names { names: names.iter().map(ToString::to_string).collect() }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn answer_in(window: &str, payload: request::Payload) -> Response {
    let bytes = Request::new(payload).for_window(window).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

/// Everything the shell was told that these tests read, in order.
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

fn forget_events() {
    EVENTS.lock().expect("a panicking test poisoned the log").clear();
}

fn events() -> Vec<Event> {
    EVENTS.lock().expect("a panicking test poisoned the log").clone()
}

/// How many conversation and message box events the shell has been sent.
fn told_count() -> usize {
    events()
        .iter()
        .filter(|told| {
            matches!(
                told.payload,
                Some(event::Payload::ConversationChanged(_) | event::Payload::MessageBoxChanged(_))
            )
        })
        .count()
}

fn conversation(window: &str) -> ConversationChanged {
    events()
        .into_iter()
        .rev()
        .filter(|told| told.window == window)
        .find_map(|told| match told.payload {
            Some(event::Payload::ConversationChanged(changed)) => Some(changed),
            _ => None,
        })
        .unwrap_or_default()
}

fn members(window: &str) -> ConversationMembersChanged {
    events()
        .into_iter()
        .rev()
        .filter(|told| told.window == window)
        .find_map(|told| match told.payload {
            Some(event::Payload::ConversationMembersChanged(changed)) => Some(changed),
            _ => None,
        })
        .unwrap_or_default()
}

fn message_box(window: &str) -> MessageBoxChanged {
    events()
        .into_iter()
        .rev()
        .filter(|told| told.window == window)
        .find_map(|told| match told.payload {
            Some(event::Payload::MessageBoxChanged(changed)) => Some(changed),
            _ => None,
        })
        .unwrap_or_default()
}

fn view(window: &str) -> ViewChanged {
    events()
        .into_iter()
        .rev()
        .filter(|told| told.window == window)
        .find_map(|told| match told.payload {
            Some(event::Payload::ViewChanged(changed)) => Some(changed),
            _ => None,
        })
        .unwrap_or_default()
}

fn latest_state(pane: &str) -> Option<String> {
    events().into_iter().rev().find_map(|told| match told.payload {
        Some(event::Payload::PaneStateChanged(state)) if state.pane_id == pane => Some(state.state),
        _ => None,
    })
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let told = Event::decode(bytes).expect("the core emits events this build can decode");
    let kept = matches!(
        told.payload,
        Some(
            event::Payload::ConversationChanged(_)
                | event::Payload::ConversationMembersChanged(_)
                | event::Payload::MessageBoxChanged(_)
                | event::Payload::ViewChanged(_)
                | event::Payload::PaneStateChanged(_)
                | event::Payload::GroupsChanged(_)
        )
    );
    if kept {
        EVENTS.lock().expect("a panicking test poisoned the log").push(told);
    }
}
