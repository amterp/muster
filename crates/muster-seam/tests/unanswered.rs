//! A change the daemon carried out and never answered about is not reported as refused.
//!
//! Every intent reaches herdr through a client with a half-second deadline, and on a loaded
//! machine herdr acts on the request and answers after the deadline has passed. What came back
//! was "the daemon did not make that change ... Nothing about the session moved", and `muster
//! pane send` exited 1 on a message the receiving agent read and replied to (kan a_2LOHfLmsL).
//! A caller told a change was refused sends it again.
//!
//! Staged with a relay in front of a real daemon that delivers the request and withholds the
//! answer (`herdr_harness::Relay`), because nothing can ask a daemon to be slow on cue. The work
//! is real: the text is read back off the daemon itself, around the relay.

use herdr_harness::{Daemon, Relay, until};
use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, SendToPane, Startup, request, response,
};
use prost::Message;
use serde_json::json;

/// Text no shell prints on its own, so finding it on the pane means the send arrived.
const MESSAGE: &str = "slot-two-was-here";

#[test]
fn a_send_the_daemon_delivered_is_not_reported_as_refused() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay = open_a_window_through(&daemon);
    let pane = the_only_pane();

    let answer = answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: MESSAGE.to_string(),
        ..SendToPane::default()
    }));

    until(
        "the text to reach the pane, read off the daemon rather than through the relay",
        || on_the_pane(&daemon).contains(MESSAGE),
        || format!("the pane shows {:?}", on_the_pane(&daemon)),
    );
    match &answer.payload {
        Some(response::Payload::Unanswered(_)) => {}
        Some(response::Payload::Failure(failure)) => panic!(
            "a send that arrived was reported as refused: {}\n  Impact: a caller told the \
             daemon did not make a change sends it again, and the pane receives the message \
             twice.",
            failure.reason
        ),
        Some(response::Payload::Ok(_)) => panic!(
            "a send whose answer never came back was reported as done. Nothing said so, and the \
             same answer for a send that did not arrive would be a lie in the other direction."
        ),
        other => panic!("a send whose answer never came back answered {other:?}"),
    }
    drop(relay);
}

#[test]
fn a_confirmed_send_whose_answer_was_lost_is_settled_by_reading_the_pane() {
    // `--confirm` reads the pane back to find out whether a send arrived. That is the question a
    // lost answer leaves open, so it is the one case where skipping the read-back because the
    // send did not succeed throws away the only certainty on offer.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay = open_a_window_through(&daemon);
    let pane = the_only_pane();

    let answer = answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: MESSAGE.to_string(),
        confirm: true,
        ..SendToPane::default()
    }));

    assert!(
        matches!(answer.payload, Some(response::Payload::Ok(_))),
        "a confirmed send that is on the pane answered {:?}",
        answer.payload
    );
    drop(relay);
}

/// A window attached to `daemon` through a relay that withholds every answer to a send.
fn open_a_window_through(daemon: &Daemon) -> Relay {
    daemon
        .call("workspace.create", &json!({ "cwd": "/tmp", "label": "unanswered", "focus": true }));
    let relay = daemon.withholding_answers_to(&["pane.send_input"]);
    let started = answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    }));
    assert_ok(&started);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    relay
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
        || "the window never listed a pane, so there was nothing to send to".to_string(),
    );
    found.expect("the wait above returns only once there is one")
}

/// What the daemon's only pane shows, asked of the daemon directly.
fn on_the_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let Some(pane) = snapshot["snapshot"]["panes"][0]["pane_id"].as_str() else {
        return String::new();
    };
    let read = daemon.call("pane.read", &json!({ "pane_id": pane, "source": "recent_unwrapped" }));
    read["read"]["text"].as_str().unwrap_or_default().to_string()
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
