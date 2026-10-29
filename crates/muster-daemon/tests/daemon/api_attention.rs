//! The API's attention topic (MIP-10, section 4): what waits on the human and their presence,
//! read by a program through the API. A control connection stands in for Muster's app, as in
//! `attention.rs`, and nothing here needs one to be open.

use std::time::Duration;

use crate::attention::{DONE, a_message_for_the_human, msg, named, report, subscribed, the_human};
use crate::support::*;
use muster_harness::ApiAsked;
use proto::msg_request::{self, Request as Asked};

fn attention(asked: &ApiAsked) -> api::Attention {
    asked.snapshot().attention.clone().expect("a subscription to attention carries its part")
}

fn asking(attention: &api::Attention) -> Vec<(String, api::Ask, bool)> {
    let asking = attention.asking.iter();
    asking.map(|asking| (asking.pane.clone(), asking.ask(), asking.unheard_helper)).collect()
}

/// Each group where something waits, as `(group, count, numbers listed)`.
fn waiting(attention: &api::Attention) -> Vec<(String, u64, Vec<u64>)> {
    let unread = attention.unread.iter();
    unread
        .map(|unread| {
            let numbers = unread.waiting.iter().map(|post| post.number).collect();
            (unread.group.clone(), unread.count, numbers)
        })
        .collect()
}

/// The next unread state of `group` that `holds` says is the one awaited.
fn unread_until(api: &mut Api, group: &str, holds: impl Fn(&api::Unread) -> bool) -> api::Unread {
    let event = api.event_matching("what waits in the group", |event| {
        matches!(
            &event.event,
            Some(api::event::Event::Unread(unread)) if unread.group == group && holds(unread)
        )
    });
    match event.event {
        Some(api::event::Event::Unread(unread)) => unread,
        other => unreachable!("matched {other:?}"),
    }
}

fn next_asking(api: &mut Api) -> api::Asking {
    let event = api.event_matching("a pane asking", |event| {
        matches!(event.event, Some(api::event::Event::Asking(_)))
    });
    match event.event {
        Some(api::event::Event::Asking(asking)) => asking,
        other => unreachable!("matched {other:?}"),
    }
}

fn next_presence(api: &mut Api) -> api::Presence {
    let event = api.event_matching("the human's presence", |event| {
        matches!(event.event, Some(api::event::Event::Presence(_)))
    });
    match event.event {
        Some(api::event::Event::Presence(presence)) => presence,
        other => unreachable!("matched {other:?}"),
    }
}

/// The card's first two proofs: with Muster's app not running, a message for the human and a
/// blocked agent are in a program's snapshot, reading them reads nothing for the human, and the
/// program connecting again after a restart finds both still waiting.
#[test]
fn with_no_app_what_waits_is_in_a_programs_snapshot_and_still_there_when_it_restarts() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    a_message_for_the_human(&mut control);

    let mut program = daemon.api();
    let first = attention(&program.subscribe(&[api::Topic::Attention]));
    assert_eq!(asking(&first), [("p1".to_string(), api::Ask::Blocked, false)]);
    let [unread] = first.unread.as_slice() else { panic!("one group waits: {first:?}") };
    let [post] = unread.waiting.as_slice() else { panic!("one message waits: {unread:?}") };
    assert_eq!((unread.group.as_str(), unread.count), ("g", 1));
    assert!(unread.place.is_some_and(|place| place < post.number), "{unread:?}");
    assert_eq!((post.author.as_str(), post.addressed, post.urgent), ("a", true, false));
    assert_eq!(first.presence, Some(api::Presence::default()), "no app, so no presence");
    drop(program);

    let mut restarted = daemon.api();
    let second = attention(&restarted.subscribe(&[api::Topic::Attention]));
    assert_eq!(second, first, "the program finds everything still waiting");
    let notices = snapshot(&mut control).human;
    assert!(notices.iter().any(|notice| notice.group == "g" && notice.count == 1), "{notices:?}");
}

/// The card's third proof: the human reading the group at the desk moves their place and empties
/// what waits there in one event, and the agent going back to work stops it asking.
#[test]
fn reading_at_the_desk_and_an_agent_going_back_to_work_clear_what_waits() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Attention]);
    daemon.set_agent_state("p1", proto::AgentState::Blocked);
    assert_eq!(next_asking(&mut program).ask(), api::Ask::Blocked);
    a_message_for_the_human(&mut control);
    let waits = unread_until(&mut program, "g", |unread| unread.count == 1);

    let mut window = daemon.connect();
    subscribed(&mut window, attending_request());
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    expect(&mut window, msg(&the_human(), read), DONE);
    let read = unread_until(&mut program, "g", |unread| unread.count == 0);
    assert!(read.waiting.is_empty(), "{read:?}");
    assert!(read.place > waits.place, "the place moved past what waited: {read:?}");

    daemon.set_agent_state("p1", proto::AgentState::Working);
    let stopped = next_asking(&mut program);
    assert_eq!((stopped.pane.as_str(), stopped.ask()), ("p1", api::Ask::Unspecified));
    let now = attention(&daemon.api().subscribe(&[api::Topic::Attention]));
    assert_eq!((asking(&now), waiting(&now)), (Vec::new(), Vec::new()));
}

/// The card's fourth proof: presence the app stops renewing, as a hung app's, reads as unknown
/// once its report runs out, and an app that is killed takes its presence with it at once.
#[test]
fn presence_reads_unknown_once_reports_stop_and_goes_with_a_killed_app() {
    let daemon = daemon();
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Attention]);

    let mut window = daemon.connect();
    subscribed(&mut window, attending_request());
    let attended = next_presence(&mut program);
    assert_eq!((attended.attended, attended.known), (true, false));
    expect(&mut window, report(true, Some(1_000), 1_500), DONE);
    let present = next_presence(&mut program);
    assert_eq!((present.known, present.in_front, present.last_input_ms), (true, true, Some(1_000)));
    let ran_out = next_presence(&mut program);
    assert_eq!((ran_out.attended, ran_out.known), (true, false), "{ran_out:?}");

    expect(&mut window, report(false, None, 60_000), DONE);
    assert!(next_presence(&mut program).known);
    drop(window);
    assert_eq!(next_presence(&mut program), api::Presence::default());
}

/// What the app showed reaches a program as information.
#[test]
fn what_the_app_showed_reaches_a_program() {
    let daemon = daemon();
    let mut program = daemon.api();
    program.subscribe(&[api::Topic::Attention]);
    let shown = proto::Shown {
        about: Some(proto::shown::About::Pane("p1".to_string())),
        shown_as: proto::ShownAs::Notification.into(),
        why: "blocked".to_string(),
    };
    expect(&mut daemon.connect(), session(proto::session_request::Request::Shown(shown)), DONE);
    let heard = program.event_matching("what was shown", |event| {
        matches!(event.event, Some(api::event::Event::Shown(_)))
    });
    let expected = api::Shown {
        about: Some(api::shown::About::Pane("p1".to_string())),
        shown_as: api::ShownAs::Notification.into(),
        why: "blocked".to_string(),
    };
    assert_eq!(heard.event, Some(api::event::Event::Shown(expected)));
}

/// What the human has read and what waits for them is one event, so a program hearing both
/// topics that carry it gets it once; a program hearing neither gets nothing of it.
#[test]
fn what_waits_is_one_event_to_a_program_hearing_either_topic_and_none_to_others() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut both = daemon.api();
    both.subscribe(&[api::Topic::Messages, api::Topic::Attention]);
    let mut neither = daemon.api();
    neither.subscribe(&[api::Topic::Machines]);
    a_message_for_the_human(&mut control);

    let first = unread_until(&mut both, "g", |unread| unread.count == 1);
    while let Some(message) = both.next_message(Duration::from_millis(200)) {
        let api::daemon_message::Message::Event(event) = &message else { continue };
        assert!(
            !matches!(&event.event, Some(api::event::Event::Unread(unread)) if *unread == first),
            "the same unread state came twice: {event:?}"
        );
    }
    assert!(neither.next_message(Duration::from_millis(200)).is_none());
}

/// However many messages wait, a group's unread state lists the newest 500 and counts them all,
/// so no frame grows with the backlog and nothing is left out unsaid.
#[test]
fn a_long_backlog_lists_the_newest_and_counts_them_all() {
    let daemon = daemon();
    let mut control = daemon.connect();
    a_message_for_the_human(&mut control);
    for each in 0..600 {
        let post = msg_request::Post {
            to: vec!["@human".to_string()],
            body: format!("update {each}"),
            ..Default::default()
        };
        expect(&mut control, msg(&named("a"), Asked::Post(post)), DONE);
    }
    let now = attention(&daemon.api().subscribe(&[api::Topic::Attention]));
    let [(group, count, numbers)] = waiting(&now).try_into().expect("one group waits");
    assert_eq!((group.as_str(), count, numbers.len()), ("g", 601, 500));
    assert!(numbers.is_sorted(), "oldest first");
    let newest = snapshot(&mut control).human[0].last;
    assert_eq!(numbers.last(), Some(&newest), "the newest are the ones listed");
}
