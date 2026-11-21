//! The API's panes topic (MIP-10 section 4): every pane in the snapshot, and an event when one
//! appears, changes in something a client reads, or closes.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::attribution::until_reported;
use crate::persistence::until_saved;
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
        effort: Some("xhigh".to_string()),
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
        (facts.context_used, facts.model.as_str(), facts.effort.as_str(), facts.cost_usd),
        (Some(42.0), "Opus", "xhigh", Some(1.25)),
        "{p1:?}"
    );
    assert_eq!(facts.waiting, "a build");
    assert_eq!(p1.session, "s-1");
    assert!(p1.session_transcript.is_none());
    assert_eq!(p1.state(), api::PaneState::Waiting, "idle, having said what it waits on");
    assert_eq!(p1.adapter(), api::Adapter::Reporting);
    let p2 = in_snapshot(&snapshot, "p2");
    assert_eq!((p2.maker.as_str(), p2.hidden), ("p1", true), "{p2:?}");
}

pub(crate) fn transcript_arguments(session: &str, directory: &str, path: &str) -> String {
    format!(
        "--agent claude --session-id {session} --session-transcript {path} \
         --session-directory {directory}"
    )
}

pub(crate) fn current_pane(daemon: &Daemon, pane: &str) -> api::Pane {
    in_snapshot(&subscribed(daemon).1, pane).clone()
}

#[test]
fn a_session_transcript_requires_the_panes_hook_and_directory() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let (mut api, initial) = subscribed(&daemon);
    let cwd = &in_snapshot(&initial, "p1").directory;
    let path = "/transcripts/hook.jsonl";
    let args = transcript_arguments("s-1", &format!("{cwd}/./"), path);

    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 1)[0], "hook 0");
    let expected = api::SessionTranscript {
        session_id: "s-1".into(),
        path: path.into(),
        directory: cwd.clone(),
    };
    let changed = api.event_matching("the session transcript", |event| {
        carried(event).is_some_and(|p| p.session_transcript.is_some())
    });
    let changed = carried(&changed).unwrap();
    assert_eq!(changed.session, "s-1");
    assert_eq!(changed.session_transcript.as_ref(), Some(&expected));
    assert_eq!(current_pane(&daemon, "p1").session_transcript, Some(expected.clone()));

    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 2)[1], "hook 0");
    let barrier = api.ask(read_pane("p1", None));
    assert!(barrier.events.iter().all(|event| carried(event).is_none()), "{:?}", barrier.events);

    for (index, (source, directory, path, status)) in [
        ("hook", "/wrong-cwd", "/transcripts/wrong.jsonl", "hook 0"),
        ("hook", cwd.as_str(), "relative.jsonl", "hook 0"),
        ("nested", cwd.as_str(), "/transcripts/nested.jsonl", "nested 1"),
    ]
    .into_iter()
    .enumerate()
    {
        let args = transcript_arguments("s-1", directory, path);
        daemon.type_into("p1", &format!("{source} {args}"));
        assert_eq!(until_reported(&daemon, index + 3)[index + 2], status);
        assert_eq!(current_pane(&daemon, "p1").session_transcript, Some(expected.clone()));
    }

    // A control client can report the session ID, as before.
    let report = pane_request::Report {
        pane: "p1".into(),
        agent: "claude".into(),
        session_id: Some("s-2".into()),
        session_transcript: Some(proto::SessionTranscript {
            session_id: "s-2".into(),
            path: "/transcripts/unattributed.jsonl".into(),
            directory: cwd.clone(),
        }),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    let replaced = api.event_matching("the next session", |event| {
        carried(event).is_some_and(|p| p.session == "s-2")
    });
    assert!(carried(&replaced).unwrap().session_transcript.is_none());

    let args = transcript_arguments("s-3", cwd, path);
    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 6)[5], "hook 0");
    assert!(current_pane(&daemon, "p1").session_transcript.is_some());
    daemon.type_into("p1", "hook --agent claude --session-source clear");
    assert_eq!(until_reported(&daemon, 7)[6], "hook 0");
    assert!(current_pane(&daemon, "p1").session_transcript.is_none());
}

#[test]
fn a_live_handoff_keeps_provenance_and_a_cold_restart_gets_a_new_report() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let cwd = current_pane(&daemon, "p1").directory;
    let args = transcript_arguments("s-1", &cwd, "/transcripts/live.jsonl");
    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 1)[0], "hook 0");
    let kept = current_pane(&daemon, "p1").session_transcript.expect("reported provenance");
    until_saved(&daemon, "s-1");

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    assert_eq!(current_pane(&daemon, "p1").session_transcript, Some(kept.clone()));

    let mut control = daemon.connect();
    let _control = restarted(&mut daemon, &mut control);
    daemon.until_agent("p1", proto::AgentState::Idle);
    let resumed = current_pane(&daemon, "p1");
    assert_eq!(resumed.session, "s-1");
    assert!(resumed.session_transcript.is_none());

    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 2)[1], "hook 0");
    assert_eq!(current_pane(&daemon, "p1").session_transcript, Some(kept));
    let next = daemon.root().join("project");
    std::fs::create_dir(&next).unwrap();
    let next = canonical(&next).display().to_string();
    daemon.type_into("p1", &format!("cd {next}"));
    until("the agent to change directory", || current_pane(&daemon, "p1").directory == next, ());
    assert!(current_pane(&daemon, "p1").session_transcript.is_none());

    let args = transcript_arguments("s-1", &next, "/transcripts/live.jsonl");
    daemon.type_into("p1", &format!("hook {args}"));
    assert_eq!(until_reported(&daemon, 3)[2], "hook 0");
    assert!(current_pane(&daemon, "p1").session_transcript.is_some());
    daemon.type_into("p1", "quit");
    until("the agent to leave", || current_pane(&daemon, "p1").harness.is_empty(), ());
    assert!(current_pane(&daemon, "p1").session_transcript.is_none());
}

/// A pane is made before it is placed, and a client hears of it once it is in its tab, so a helper
/// made into a hidden tab never reads as shown.
#[test]
fn a_helper_made_hidden_appears_hidden() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let (mut api, _) = subscribed(&daemon);

    make(&mut control, helper("p2", "p1", in_new_hidden_tab("t2")));

    let appeared = api.event_matching("p2 to appear", |event| {
        matches!(&event.event, Some(Event::PaneAppeared(_)))
            && carried(event).is_some_and(|held| held.pane == "p2")
    });
    let p2 = carried(&appeared).expect("the pane");
    assert!(p2.hidden, "a hidden helper appeared as shown: {p2:?}");
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
