//! A daemon slow to answer does not hold the window closed.
//!
//! Every daemon a config names used to be attached, one after another, before the shell had made
//! a window, so a devenv slow to answer kept the window from appearing for up to a minute and a
//! half (kan a_2Y3LSOogS). Staged with a relay in front of a real daemon that holds back its
//! answer to the window's subscribe (`muster_harness::Relay`), because nothing can ask a daemon
//! to be slow on cue. Local rather than over ssh, because the rule is the same for every daemon
//! and the transport adds nothing to it.

use std::time::{Duration, Instant};

use muster::proto::{OpenWindow, ReadWindow, Request, Response, Startup, request, response};
use muster_daemon_proto::{self as daemon_proto, session_request};
use muster_harness::{Daemon, until};
use prost::Message;

/// How long the relay holds back the daemon's state: longer than anyone would wait for a
/// window, shorter than the window's own patience for a first snapshot.
const SLOW: Duration = Duration::from_secs(5);

#[test]
fn a_slow_daemon_does_not_hold_the_window_closed() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let relay = daemon.delaying_answers_where(subscribes, SLOW);

    let asked = Instant::now();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    let started_in = asked.elapsed();
    assert!(
        started_in < Duration::from_secs(3),
        "starting took {started_in:?} waiting for a daemon {SLOW:?} slow to answer.\n  Impact: \
         the window does not appear until every daemon it names has answered."
    );

    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the slow daemon's pane to arrive in the open window",
        || listed_panes() == 1,
        || format!("the window lists {} panes", listed_panes()),
    );
    drop(relay);
}

/// The window's subscribe, whose answer carries the daemon's state.
fn subscribes(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(session_request::Request::Subscribe(_)),
        }))
    )
}

fn listed_panes() -> usize {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window.panes.len(),
        _ => 0,
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
