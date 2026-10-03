//! A change the daemon carried out and never answered about is not reported as refused.
//!
//! Every intent reaches the daemon through a client with a deadline, and on a loaded machine the
//! daemon acts on a request and answers after the deadline has passed. What came back once was
//! "the daemon did not make that change ... Nothing about the session moved", about a change
//! that had been made (kan a_2LOHfLmsL). A caller told a change was refused makes it again.
//!
//! Staged with a relay in front of a real daemon that delivers the request and withholds the
//! answer (`muster_harness::Relay`), because nothing can ask a daemon to be slow on cue. The work
//! is real, and every event it produces still arrives: what the daemon holds is read off the
//! daemon itself, around the relay.

use std::path::PathBuf;

use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, SplitPane, Startup, request, response,
};
use muster_daemon_proto::{self as daemon_proto, input_event, pane_request, placement};
use muster_harness::requests::snapshot;
use muster_harness::{Daemon, Input, Relay, until, until_some};
use prost::Message;

#[test]
fn a_pane_made_by_a_split_answered_too_late_keeps_its_name() {
    // The name a pane is told is minted before the split and sent with it, and the window once
    // learned which pane a split made only from the daemon's answer. An answer that came after
    // the window stopped waiting was thrown away with the connection, and the pane arrived on
    // events under a name minted on sight while the process inside it was started with the
    // first one. An agent there then got "no pane called ..." for its own pane (kan a_2P65ttmCu).
    //
    // Takes the client's whole patience, ten seconds, because an answer the window has stopped
    // waiting for is the case under test and nothing shortens that wait from outside.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let relay = open_a_window_through(daemon.withholding_answers_where(makes_a_split));
    let pane = the_only_pane();
    let before = daemon_panes(&daemon);

    let answer = answer(request::Payload::SplitPane(SplitPane {
        pane_id: pane,
        side: "right".to_string(),
        ..SplitPane::default()
    }));
    assert!(
        matches!(answer.payload, Some(response::Payload::Unanswered(_))),
        "a split whose answer never came back answered {:?}",
        answer.payload
    );

    // Read out of the pane's own environment, around the relay, because that is the name
    // everything run inside it will use.
    let made = until_some("the daemon to hold the pane the split made", || {
        daemon_panes(&daemon).into_iter().find(|pane| !before.contains(pane))
    });
    let told = told_its_name(&daemon, &made);

    until(
        "the window to list the new pane under the name it was told",
        || {
            let listed = listed_panes();
            listed.len() == 2 && listed.contains(&told)
        },
        || {
            format!(
                "the pane was told it is {told} and the window lists {:?}.\n  Impact: every \
                 `muster` command run in that pane is refused for a pane that does not exist.",
                listed_panes()
            )
        },
    );
    drop(relay);
}

/// A request making a pane beside another, which is what a split is. The window's first tab is
/// a pane made in a new tab, and is answered, so the window opens as it always does.
fn makes_a_split(request: &daemon_proto::Request) -> bool {
    let Some(daemon_proto::request::Service::Pane(daemon_proto::PaneRequest {
        request: Some(pane_request::Request::Create(create)),
    })) = &request.service
    else {
        return false;
    };
    matches!(
        create.placement.as_ref().and_then(|placement| placement.r#where.as_ref()),
        Some(placement::Where::Beside(_))
    )
}

/// A window attached to a daemon through `relay`.
fn open_a_window_through(relay: Relay) -> Relay {
    let started = answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    }));
    assert_ok(&started);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    relay
}

/// Every pane the window lists, by Muster's names for them.
fn listed_panes() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.into_iter().map(|pane| pane.pane_id).collect()
        }
        _ => Vec::new(),
    }
}

/// Every pane the daemon holds, asked of the daemon directly.
fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect()
}

/// What a pane's shell says `$MUSTER_PANE` is, written to a file rather than read off its screen
/// so the answer is not wrapped or echoed.
fn told_its_name(daemon: &Daemon, pane: &str) -> String {
    let dump =
        PathBuf::from(format!("/tmp/muster-test/unanswered-name-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&dump);
    Input::connect(daemon.socket_path()).send(
        pane,
        input_event::Input::Send(input_event::Send {
            text: format!("printf '%s' \"$MUSTER_PANE\" > {}", dump.display()),
            enter: true,
        }),
    );
    let written = until_some("the shell in the new pane to write out its name", || {
        std::fs::read_to_string(&dump).ok().filter(|text| !text.is_empty())
    });
    let _ = std::fs::remove_file(&dump);
    written.trim().to_string()
}

/// The window's only pane, by Muster's name for it.
fn the_only_pane() -> String {
    let mut found = None;
    until(
        "the window to hold the daemon's pane",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(ReadWindow {})).payload
            else {
                return false;
            };
            found = window.panes.first().map(|pane| pane.pane_id.clone());
            found.is_some()
        },
        || "the window never listed a pane, so there was nothing to split".to_string(),
    );
    found.expect("the wait above returns only once there is one")
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
