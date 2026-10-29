//! The API's panes topic (MIP-10 section 4): every pane in the snapshot, and an event when one
//! appears, changes in something a client reads, or closes.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::resume::{reported, restarted};
use crate::support::*;
use api::event::Event;
use muster_harness::Input;
use proto::input_event::{self, Input as Typed};
use proto::pane_request;

fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970");
    u64::try_from(since.as_millis()).expect("milliseconds fit")
}

fn subscribed(daemon: &Daemon) -> (Api, api::Snapshot) {
    let mut api = daemon.api();
    let snapshot = api.subscribe(&[api::Topic::Panes]).snapshot().clone();
    (api, snapshot)
}

fn in_snapshot<'a>(snapshot: &'a api::Snapshot, pane: &str) -> &'a api::Pane {
    let found = snapshot.panes.iter().find(|held| held.pane == pane);
    found.unwrap_or_else(|| panic!("the snapshot holds no {pane}: {snapshot:?}"))
}

/// The pane a pane event carries, appeared or changed.
fn carried(event: &api::Event) -> Option<&api::Pane> {
    match event.event.as_ref()? {
        Event::PaneAppeared(api::PaneAppeared { pane })
        | Event::PaneChanged(api::PaneChanged { pane }) => pane.as_deref(),
        _ => None,
    }
}

fn changed_to(api: &mut Api, pane: &str, state: api::PaneState) -> api::Pane {
    let event = api.event_matching(&format!("{pane} to change to {state:?}"), |event| {
        matches!(&event.event, Some(Event::PaneChanged(_)))
            && carried(event).is_some_and(|held| held.pane == pane && held.state() == state)
    });
    carried(&event).cloned().expect("a changed pane")
}

#[test]
fn a_client_sees_a_pane_appear_change_state_and_close() {
    let daemon = Daemon::start_detecting();
    let (mut api, snapshot) = subscribed(&daemon);
    assert!(snapshot.panes.is_empty(), "{snapshot:?}");
    let mut control = daemon.connect();

    make(&mut control, create("p1", in_new_tab("t1")));
    let appeared = api.event_matching("p1 to appear", |event| {
        matches!(&event.event, Some(Event::PaneAppeared(_)))
    });
    let p1 = carried(&appeared).expect("the pane");
    assert_eq!(p1.pane, "p1");
    assert!(!p1.callsign.is_empty() && !p1.directory.is_empty(), "{p1:?}");
    assert_eq!((p1.state(), p1.harness.as_str(), p1.hidden), (api::PaneState::Unknown, "", false));
    assert_eq!(p1.source, appeared.source, "a pane names the daemon its event came from");

    daemon.run_agent("p1");
    changed_to(&mut api, "p1", api::PaneState::Idle);
    let before = now_ms();
    daemon.set_agent_state("p1", proto::AgentState::Working);
    let after = now_ms();
    let working = changed_to(&mut api, "p1", api::PaneState::Working);
    assert_eq!(working.harness, "claude");
    assert!(
        working.since_ms.is_some_and(|since| (before..=after).contains(&since)),
        "the agent went to work between {before} and {after}, and the API says {:?}",
        working.since_ms
    );

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    api.event_matching(
        "p1 to close",
        |event| matches!(&event.event, Some(Event::PaneClosed(closed)) if closed.pane == "p1"),
    );
}

/// What an agent says about itself, and what made it, reach the snapshot as typed fields.
#[test]
fn a_snapshot_carries_what_a_pane_says_and_who_made_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        context_used: Some(42.0),
        model: Some("Opus".to_string()),
        cost_usd: Some(1.25),
        waiting: Some("a build".to_string()),
        session_id: Some("s-1".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    make(&mut control, helper("p2", "p1", in_new_hidden_tab("t2")));

    let (_api, snapshot) = subscribed(&daemon);

    let p1 = in_snapshot(&snapshot, "p1");
    let facts = p1.facts.clone().unwrap_or_default();
    assert_eq!(
        (facts.context_used, facts.model.as_str(), facts.cost_usd, facts.waiting.as_str()),
        (Some(42.0), "Opus", Some(1.25), "a build"),
        "{p1:?}"
    );
    assert_eq!(p1.session, "s-1");
    assert_eq!(p1.state(), api::PaneState::Waiting, "idle, having said what it waits on");
    assert_eq!(p1.adapter(), api::Adapter::Reporting);
    let p2 = in_snapshot(&snapshot, "p2");
    assert_eq!((p2.maker.as_str(), p2.hidden), ("p1", true), "{p2:?}");
}

/// A harness redraws its title while it works, so a title is carried but never news on its own.
/// Proved without waiting: every pane event up to the rename that followed a new title changed
/// something else, and the rename's carries the title.
#[test]
fn a_title_alone_raises_no_event_and_the_next_one_carries_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let (mut api, subscribed_to) = subscribed(&daemon);
    let mut last = in_snapshot(&subscribed_to, "p1").clone();

    let mut input = Input::connect(daemon.socket_path());
    input.send(
        "p1",
        Typed::Send(input_event::Send {
            text: "printf '\\033]0;spun\\007'".to_string(),
            enter: true,
            ..Default::default()
        }),
    );
    until(
        "the daemon to hold p1's new title",
        || {
            snapshot(&mut control)
                .panes
                .iter()
                .any(|held| held.pane == "p1" && held.title == "spun")
        },
        || "it never did".to_string(),
    );
    let rename = pane_request::Rename { pane: "p1".to_string(), label: Some("named".to_string()) };
    expect(&mut control, pane(pane_request::Request::Rename(rename)), proto::Outcome::Done);

    loop {
        let event = api.event_matching("a pane event", |event| carried(event).is_some());
        let now = carried(&event).cloned().expect("a pane");
        assert_ne!(
            api::Pane { title: last.title.clone(), ..now.clone() },
            last,
            "an event changed nothing but the title"
        );
        if now.name == "named" {
            assert_eq!(now.title, "spun", "the rename's event lost the title: {now:?}");
            break;
        }
        last = now;
    }
}

/// A resumed agent's state began before the restart, which the API says it does not know.
#[test]
fn a_restarted_daemon_serves_a_resumed_agent_without_a_since() {
    let (mut daemon, mut control) = reported("", "s-1");
    let _control = restarted(&mut daemon, &mut control);
    daemon.until_agent("p1", proto::AgentState::Idle);

    let (mut api, snapshot) = subscribed(&daemon);
    let p1 = in_snapshot(&snapshot, "p1");
    assert_eq!((p1.state(), p1.since_ms), (api::PaneState::Idle, None), "{p1:?}");

    daemon.set_agent_state("p1", proto::AgentState::Working);
    let working = changed_to(&mut api, "p1", api::PaneState::Working);
    assert!(working.since_ms.is_some(), "a change the daemon saw has a time: {working:?}");
}

#[test]
fn a_handoff_keeps_since_for_the_next_subscription() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Working);
    let (_api, before) = subscribed(&daemon);
    let since = in_snapshot(&before, "p1").since_ms;
    assert!(since.is_some(), "{before:?}");

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let (_api, after) = subscribed(&daemon);
    assert_eq!(in_snapshot(&after, "p1").since_ms, since, "the handoff lost when work began");
}
