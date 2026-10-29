//! Posting and acknowledging through the API (MIP-10, sections 5 and 7): a program posts as the
//! human under an operation id, so asking again never posts twice, and acknowledges only the
//! messages it showed. A control connection stands in for the desk.

use crate::attention::{DONE, join, msg, named, the_human, until_event};
use crate::support::*;
use proto::msg_request::{self, Request as Asked};

/// A program called `foreman`, as Foreman's server would be.
fn foreman(daemon: &Daemon) -> Api {
    let client = api::Client { name: "foreman".to_string(), version: String::new() };
    Api::connect_as(daemon.socket_path(), client)
}

/// The human and `a` in `g`.
fn crew(daemon: &Daemon) -> Control {
    let mut control = daemon.connect();
    join(&mut control, &the_human(), "@human", "g");
    join(&mut control, &named("a"), "a", "g");
    control
}

fn post_as_human(id: &str, body: &str) -> api::NewPost {
    api::NewPost {
        operation: id.to_string(),
        group: "g".to_string(),
        to: vec!["a".to_string()],
        body: body.to_string(),
        ..api::NewPost::default()
    }
}

/// The number `poster` posted `post` as, in `g`.
fn posted_by(control: &mut Control, poster: &msg_request::Caller, post: msg_request::Post) -> u64 {
    let answered = expect(control, msg(poster, Asked::Post(post)), DONE).answer;
    match answered.detail {
        Some(proto::answer::Detail::Msg(proto::MsgAnswer {
            answer: Some(proto::msg_answer::Answer::Posted(answer)),
            ..
        })) => answer.seq,
        other => panic!("a post answered with {other:?}"),
    }
}

/// `a`'s question to the human in `g`, by its number, once `a` has read what it was sent.
fn a_question(control: &mut Control) -> u64 {
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(control, msg(&named("a"), read), DONE);
    let post = msg_request::Post {
        group: Some("g".to_string()),
        to: vec!["@human".to_string()],
        body: "Ship it?".to_string(),
        answers: vec!["Yes".to_string(), "No".to_string()],
        ..Default::default()
    };
    posted_by(control, &named("a"), post)
}

fn for_the_human(control: &mut Control, body: &str) -> u64 {
    let post = msg_request::Post {
        group: Some("g".to_string()),
        to: vec!["@human".to_string()],
        body: body.to_string(),
        ..Default::default()
    };
    posted_by(control, &named("a"), post)
}

/// Every message in `g`, as `(number, author, via)`.
fn messages(control: &mut Control) -> Vec<(u64, String, Option<String>)> {
    let log = Asked::Log(msg_request::Log { group: "g".to_string(), ..Default::default() });
    let answered = expect(control, msg(&named("a"), log), DONE).answer;
    let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
        answer: Some(proto::msg_answer::Answer::Entries(entries)),
        ..
    })) = answered.detail
    else {
        panic!("a log answered with {:?}", answered.detail)
    };
    let entries = entries.groups.into_iter().flat_map(|group| group.entries);
    entries
        .filter_map(|entry| match entry.what {
            Some(proto::msg_answer::entry::What::Message(message)) => {
                Some((entry.seq, message.author, message.via))
            }
            _ => None,
        })
        .collect()
}

/// What waits for the human in `g` at the desk: `(count, first)`.
fn at_the_desk(control: &mut Control) -> (u64, u64) {
    let held = snapshot(control);
    let notice = held.human.iter().find(|notice| notice.group == "g").expect("the human is in g");
    (notice.count, notice.first)
}

#[test]
fn a_programs_post_is_logged_as_the_humans_with_the_program_beside_it() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    program.subscribe(&[api::Topic::Messages]);

    let asked = program.ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
    let number = asked.posted().number;
    assert_eq!(asked.posted().group, "g");
    let logged = asked.events.iter().find_map(|event| match &event.event {
        Some(api::event::Event::Logged(logged)) => Some((event, logged)),
        _ => None,
    });
    let (event, logged) = logged.expect("the post's entry arrives before the answer");
    assert_eq!(event.correlation, asked.answer.correlation, "the post caused it");
    let Some(api::entry::What::Post(post)) = logged.entry.as_ref().and_then(|e| e.what.clone())
    else {
        panic!("{logged:?}")
    };
    assert_eq!((post.author.as_str(), post.via.as_deref()), ("@human", Some("foreman")));
    assert_eq!(
        messages(&mut control),
        [(number, "@human".to_string(), Some("foreman".to_string()))]
    );
}

#[test]
fn asking_again_under_an_id_never_posts_twice_and_another_request_cannot_reuse_it() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    let first = program.ask(post(post_as_human("op-1", "ship it")));
    let again = program.ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(again.posted(), first.posted());

    let reused = program.ask(post(post_as_human("op-1", "hold it")));
    assert_eq!(
        (reused.outcome(), reused.refusal()),
        (api::Outcome::Refused, api::Refusal::OperationReused)
    );
    let unnamed = program.ask(post(post_as_human("", "ship it")));
    assert_eq!(
        (unnamed.outcome(), unnamed.refusal()),
        (api::Outcome::Refused, api::Refusal::Invalid)
    );
    assert_eq!(messages(&mut control).len(), 1);
}

/// The id outlives the group its post was in, and a handoff: during it, and in the daemon that
/// took over, which read it back from the logs.
#[test]
fn a_repeat_is_answered_after_its_group_is_deleted_and_during_and_after_a_handoff() {
    let mut daemon = daemon_with(&[("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold")]);
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    let first = program.ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(first.outcome(), api::Outcome::Done, "{:?}", first.answer);
    let delete = Asked::GroupDelete(msg_request::GroupDelete { group: "g".to_string() });
    expect(&mut control, msg(&named("a"), delete), DONE);

    let again = program.ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(again.outcome(), api::Outcome::AlreadySo, "{:?}", again.answer);
    assert_eq!(again.posted(), first.posted());

    let replacing = daemon.start_replacing(None);
    daemon.paused();
    let during = program.ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(during.outcome(), api::Outcome::AlreadySo, "{:?}", during.answer);
    let new = program.ask(post(post_as_human("op-2", "and this")));
    assert_eq!((new.outcome(), new.refusal()), (api::Outcome::Refused, api::Refusal::Unavailable));
    daemon.resume();
    let answer = daemon.finish_replacing(replacing);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let after = foreman(&daemon).ask(post(post_as_human("op-1", "ship it")));
    assert_eq!(after.outcome(), api::Outcome::AlreadySo, "{:?}", after.answer);
    assert_eq!(after.posted(), first.posted());
}

#[test]
fn a_program_posts_as_the_participant_it_names() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let asked = api::NewPost {
        to: vec!["@human".to_string()],
        r#as: "a".to_string(),
        ..post_as_human("op-1", "done")
    };
    let posted = foreman(&daemon).ask(post(asked));
    assert_eq!(posted.outcome(), api::Outcome::Done, "{:?}", posted.answer);
    assert_eq!(messages(&mut control)[0].1, "a");
    assert_eq!(at_the_desk(&mut control).0, 1, "a's post waits for the human");
}

/// The card's proof: the desk and a program answering one question leave one answer, whichever
/// comes first, and the second is refused for being answered.
#[test]
fn the_desk_and_a_program_answering_one_question_leave_one_answer() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    for first_by_the_program in [true, false] {
        let question = a_question(&mut control);
        let answering = |answer: &str| api::NewPost {
            answering: Some(api::Answering { question, answer: answer.to_string() }),
            ..post_as_human(&format!("op-{question}"), "")
        };
        let at_the_desk = Asked::Post(msg_request::Post {
            group: Some("g".to_string()),
            to: vec!["a".to_string()],
            body: "Answer: No".to_string(),
            answering: Some(msg_request::Answering { question, answer: "No".to_string() }),
            ..Default::default()
        });
        if first_by_the_program {
            let answered = program.ask(post(answering("Yes")));
            assert_eq!(answered.outcome(), api::Outcome::Done, "{:?}", answered.answer);
            let refused =
                expect(&mut control, msg(&the_human(), at_the_desk), proto::Outcome::Refused);
            assert!(
                refused.answer.reason.contains("answered already"),
                "{}",
                refused.answer.reason
            );
        } else {
            expect(&mut control, msg(&the_human(), at_the_desk), DONE);
            let refused = program.ask(post(answering("Yes")));
            assert_eq!(
                (refused.outcome(), refused.refusal()),
                (api::Outcome::Refused, api::Refusal::Answered),
                "{:?}",
                refused.answer
            );
        }
    }
}

/// MIP-10, section 7: a question answered by a program comes down from the desk's banner without
/// the group being read, so the window's notice says it is answered.
#[test]
fn a_program_answering_tells_the_desk_the_question_is_answered() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut window = daemon.connect();
    subscribed_window(&mut window);
    let question = a_question(&mut control);
    let answering = api::NewPost {
        answering: Some(api::Answering { question, answer: "Yes".to_string() }),
        ..post_as_human("op-1", "")
    };
    let answered = foreman(&daemon).ask(post(answering));
    assert_eq!(answered.outcome(), api::Outcome::Done, "{:?}", answered.answer);
    let notice = until_event(&mut window, |event| match event {
        proto::event::Event::HumanNotice(notice) if !notice.answered.is_empty() => Some(notice),
        _ => None,
    });
    assert_eq!((notice.answered, notice.count), (vec![question], 1));
}

/// MIP-9, section 6: an answer reads the same whichever door it came through. A program sends
/// the answer it picks and any note, and the daemon composes the body as `muster msg post
/// --answer` does, for whoever asked unless the post names whom it is for.
#[test]
fn a_programs_answer_is_composed_as_the_clis_and_goes_to_the_asker() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    for (note, body) in [("", "Answer: Yes"), ("ship it today", "Answer: Yes\n\nship it today")] {
        let question = a_question(&mut control);
        let answering = api::NewPost {
            to: Vec::new(),
            answering: Some(api::Answering { question, answer: "Yes".to_string() }),
            ..post_as_human(&format!("op-{question}"), note)
        };
        let answered = program.ask(post(answering));
        assert_eq!(answered.outcome(), api::Outcome::Done, "{:?}", answered.answer);
        let number = answered.posted().number;
        let log = Asked::Log(msg_request::Log { group: "g".to_string(), ..Default::default() });
        let logged = expect(&mut control, msg(&named("a"), log), DONE).answer;
        let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
            answer: Some(proto::msg_answer::Answer::Entries(entries)),
            ..
        })) = logged.detail
        else {
            panic!("a log answered with {:?}", logged.detail)
        };
        let entry = entries.groups.into_iter().flat_map(|group| group.entries);
        let message = entry
            .filter(|entry| entry.seq == number)
            .find_map(|entry| match entry.what {
                Some(proto::msg_answer::entry::What::Message(message)) => Some(message),
                _ => None,
            })
            .expect("the answer is logged");
        assert_eq!(
            (message.body.as_str(), message.to.as_slice()),
            (body, ["a".to_string()].as_slice())
        );
    }
}

fn subscribed_window(window: &mut Control) {
    expect(window, attending_request(), DONE);
}

/// The card's proof: a program acknowledging the message it showed while a newer one arrives
/// leaves the newer one unread at the desk, and never moves the human's place back.
#[test]
fn acknowledging_what_was_shown_leaves_what_arrived_since_unread_at_the_desk() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    let mut program = foreman(&daemon);
    program.subscribe(&[api::Topic::Attention]);
    let shown = for_the_human(&mut control, "the tenth");
    let since = for_the_human(&mut control, "the eleventh");

    let acked = program.ask(ack("g", shown));
    assert_eq!(acked.outcome(), api::Outcome::Done, "{:?}", acked.answer);
    let unread = acked.events.iter().rev().find_map(|event| match &event.event {
        Some(api::event::Event::Unread(unread)) if unread.group == "g" => Some(unread.clone()),
        _ => None,
    });
    let unread = unread.expect("the place's move arrives before the answer");
    assert_eq!(unread.place, Some(shown));
    assert_eq!(unread.waiting.iter().map(|post| post.number).collect::<Vec<_>>(), [since]);
    assert_eq!(at_the_desk(&mut control), (1, since));

    let back = program.ask(ack("g", shown - 1));
    assert_eq!(back.outcome(), api::Outcome::AlreadySo, "{:?}", back.answer);
    let past = program.ask(ack("g", since + 1));
    assert_eq!((past.outcome(), past.refusal()), (api::Outcome::Refused, api::Refusal::Invalid));
    let elsewhere = program.ask(ack("nowhere", 1));
    assert_eq!(elsewhere.outcome(), api::Outcome::Refused, "{:?}", elsewhere.answer);
    assert_eq!(at_the_desk(&mut control), (1, since));

    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(&mut control, msg(&the_human(), read), DONE);
    assert_eq!(at_the_desk(&mut control).0, 0, "a read at the desk still reads to the end");
}

/// A refused post leaves nothing behind: a reply to a question in a pair it would have made.
#[test]
fn a_refused_reply_makes_no_group() {
    let daemon = daemon();
    let mut control = crew(&daemon);
    join(&mut control, &named("b"), "b", "other");
    let mut program = foreman(&daemon);
    let groups = program.subscribe(&[api::Topic::Messages]).snapshot().groups.len();
    let reply = api::NewPost {
        group: String::new(),
        to: vec!["b".to_string()],
        answering: Some(api::Answering { question: 3, answer: "Yes".to_string() }),
        ..post_as_human("op-1", "")
    };
    let refused = program.ask(post(reply));
    assert_eq!(
        (refused.outcome(), refused.refusal()),
        (api::Outcome::Refused, api::Refusal::NotFound)
    );
    assert!(
        !refused
            .events
            .iter()
            .any(|event| matches!(event.event, Some(api::event::Event::GroupMade(_)))),
        "{:?}",
        refused.events
    );
    assert_eq!(program.subscribe(&[api::Topic::Messages]).snapshot().groups.len(), groups);
}
