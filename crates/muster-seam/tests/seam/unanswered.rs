//! A change the daemon carried out and never answered about is not reported as refused.
//!
//! Every intent reaches herdr through a client with a half-second deadline, and on a loaded
//! machine herdr acts on the request and answers after the deadline has passed. What came back
//! was "the daemon did not make that change ... Nothing about the session moved", and `muster
//! pane send` exited 1 on a message the receiving agent read and replied to (kan a_2LOHfLmsL).
//! A caller told a change was refused sends it again.
//!
//! Nor as done. `pane send --enter` is two requests, the text and then Return, and a Return whose
//! answer was lost exited 0 with the text possibly sitting on the prompt unsent (kan a_2P65u60Qp).
//!
//! Staged with a relay in front of a real daemon that delivers the request and withholds the
//! answer (`herdr_harness::Relay`), because nothing can ask a daemon to be slow on cue. The work
//! is real: the text is read back off the daemon itself, around the relay.

use herdr_harness::{Daemon, Relay, until, until_some};
use std::path::PathBuf;
use std::time::Duration;

use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, SendToPane, SplitPane, Startup, request, response,
};
use prost::Message;
use serde_json::{Value, json};

/// How late the split's answer arrives: well past the half second a window gives a daemon.
const ANSWERED_AFTER: Duration = Duration::from_secs(2);

/// Text no shell prints on its own, so finding it on the pane means the send arrived.
const MESSAGE: &str = "slot-two-was-here";

/// A command whose output is not its own text, so finding the output means Return was pressed.
const COMMAND: &str = "printf 'slot-%s\\n' two-submitted";
const SUBMITTED: &str = "slot-two-submitted";

#[test]
fn a_send_the_daemon_delivered_is_not_reported_as_refused() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay = open_a_window_through(&daemon, daemon.withholding_answers_to(&["pane.send_input"]));
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
    let relay = open_a_window_through(&daemon, daemon.withholding_answers_to(&["pane.send_input"]));
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

#[test]
fn a_send_whose_return_went_unanswered_is_not_reported_as_submitted() {
    // The text is answered and only the Return after it goes missing, so the caller cannot know
    // whether what it sent was submitted - and exit 0 says it was (kan a_2P65u60Qp).
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay = open_a_window_through(&daemon, daemon.withholding_answers_where(presses_return));
    let pane = the_only_pane();

    let answer = answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: COMMAND.to_string(),
        enter: true,
        ..SendToPane::default()
    }));

    until(
        "the command's output to reach the pane, which only a pressed Return produces",
        || on_the_pane(&daemon).contains(SUBMITTED),
        || format!("the pane shows {:?}", on_the_pane(&daemon)),
    );
    assert_unanswered(&answer);
    drop(relay);
}

#[test]
fn a_confirmed_send_whose_return_went_unanswered_is_not_settled_by_the_read_back() {
    // `--confirm` finds the text on the pane, and the text is on a pane whether or not Return
    // submitted it. So reading it back settles arrival and not the Return.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay = open_a_window_through(&daemon, daemon.withholding_answers_where(presses_return));
    let pane = the_only_pane();

    let answer = answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: COMMAND.to_string(),
        enter: true,
        confirm: true,
        ..SendToPane::default()
    }));

    assert_unanswered(&answer);
    drop(relay);
}

/// A request that presses Return rather than typing text. Both go out as `pane.send_input`.
fn presses_return(request: &Value) -> bool {
    request["method"] == "pane.send_input" && request["params"].get("keys").is_some()
}

fn assert_unanswered(answer: &Response) {
    match &answer.payload {
        Some(response::Payload::Unanswered(_)) => {}
        Some(response::Payload::Ok(_)) => panic!(
            "a send whose Return was never answered was reported as submitted.\n  Impact: exit 0 \
             is the only thing a script reads, and the text may be sitting on the prompt unsent."
        ),
        other => panic!("a send whose Return was never answered answered {other:?}"),
    }
}

#[test]
fn a_pane_made_by_a_split_answered_too_late_keeps_its_name() {
    // The name a pane is told is minted before the split and sent with it, and herdr says which
    // pane it made only in its answer. An answer that comes after the window stopped waiting
    // used to be thrown away with the connection, and the pane arrived on events under a name
    // minted on sight while the process inside it was started with the first one. An agent
    // there then got "no pane called ..." for its own pane (kan a_2P65ttmCu).
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let relay =
        open_a_window_through(&daemon, daemon.delaying_answers_to(&["pane.split"], ANSWERED_AFTER));
    let pane = the_only_pane();
    let before = daemon_panes(&daemon);

    let answer = answer(request::Payload::SplitPane(SplitPane {
        pane_id: pane,
        side: "right".to_string(),
        ..SplitPane::default()
    }));
    assert!(
        matches!(answer.payload, Some(response::Payload::Unanswered(_))),
        "a split answered after the window's deadline answered {:?}",
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

/// A window attached to `daemon` through `relay`.
fn open_a_window_through(daemon: &Daemon, relay: Relay) -> Relay {
    daemon
        .call("workspace.create", &json!({ "cwd": "/tmp", "label": "unanswered", "focus": true }));
    let started = answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    }));
    assert_ok(&started);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
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

/// herdr's ids for every pane the daemon holds, asked of the daemon directly.
fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    let listed = daemon.call("pane.list", &json!({}));
    listed["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
        .collect()
}

/// What a pane's shell says `$MUSTER_PANE` is, written to a file rather than read off its screen
/// so the answer is not wrapped or echoed.
fn told_its_name(daemon: &Daemon, pane: &str) -> String {
    let dump =
        PathBuf::from(format!("/tmp/muster-test/unanswered-name-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&dump);
    daemon.call(
        "pane.send_text",
        &json!({
            "pane_id": pane,
            "text": format!("printf '%s' \"$MUSTER_PANE\" > {}\n", dump.display()),
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
