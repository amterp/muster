//! When the daemon saw a pane's agent enter its current state (`Pane.state_since_ms`): dated as
//! the daemon sees each change, kept across a handoff, and unknown for an agent a restart
//! resumed, rather than dated by when anybody first looked.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::persistence::record;
use crate::resume::{reported, restarted};
use crate::support::*;
use proto::AgentState::{Idle, Working};
use proto::pane_request;

fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970");
    u64::try_from(since.as_millis()).expect("milliseconds fit")
}

fn since(control: &mut Control, pane: &str) -> Option<u64> {
    record(&snapshot(control), pane).state_since_ms
}

/// Sets the fake agent in `pane` to `state` and says when the daemon dated it, failing unless
/// that was while the change was being made.
fn dated_change(
    daemon: &Daemon,
    control: &mut Control,
    pane: &str,
    state: proto::AgentState,
) -> u64 {
    let before = now_ms();
    daemon.set_agent_state(pane, state);
    let after = now_ms();
    let since = since(control, pane);
    assert!(
        since.is_some_and(|since| (before..=after).contains(&since)),
        "{pane}'s agent became {state:?} between {before} and {after}, and the daemon dates it \
         {since:?}, so a client subtracting it from now gets the wrong length of time"
    );
    since.unwrap_or_default()
}

#[test]
fn each_change_of_state_is_dated_as_the_daemon_saw_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    let before = now_ms();
    make(&mut control, create("p1", in_new_tab("t1")));
    let made = since(&mut control, "p1");
    assert!(
        made.is_some_and(|made| made >= before),
        "the daemon made the pane and saw it start with no agent, yet dates that {made:?}"
    );
    daemon.run_agent("p1");

    let working = dated_change(&daemon, &mut control, "p1", Working);

    // What the agent says about itself changes the record, and not its state.
    let report = pane_request::Report {
        pane: "p1".to_string(),
        context_used: Some(42.0),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    assert_eq!(since(&mut control, "p1"), Some(working), "a report of context moved `since`");

    let idle = dated_change(&daemon, &mut control, "p1", Idle);
    assert!(idle >= working, "the daemon's clock ran backwards: {working} then {idle}");
}

#[test]
fn a_handoff_keeps_when_the_agent_entered_its_state() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let working = dated_change(&daemon, &mut control, "p1", Working);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let mut control = daemon.connect();
    assert_eq!(
        since(&mut control, "p1"),
        Some(working),
        "the new daemon lost when the agent started working, though the agent never stopped"
    );
}

/// The resumed agent was idle in its session before the restart, which the new daemon cannot
/// say when began, so it says nothing until it sees the agent change.
#[test]
fn a_restarted_daemon_does_not_date_the_state_its_resumed_agent_was_in() {
    let (mut daemon, mut control) = reported("", "s-1");

    let mut control = restarted(&mut daemon, &mut control);
    daemon.until_agent("p1", Idle);
    assert_eq!(
        since(&mut control, "p1"),
        None,
        "the daemon dated a state it did not see begin, so it reads as begun at the restart"
    );

    dated_change(&daemon, &mut control, "p1", Working);
}
