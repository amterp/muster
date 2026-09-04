//! What `muster daemons` answers, and the one thing only a window can add to it.
//!
//! The record and the dial are the herdr adapter's and are tested there. What this is about is
//! the half a file cannot hold: whether the window answering is attached to the daemon in the
//! row. That is the field the decision turns on - ending a daemon nothing is attached to costs
//! whoever started it, and ending the one this window is drawing costs the person reading the
//! answer - and it is knowable only where the composition is.
//!
//! A real daemon, because `attached_here` is true when a window is really following one.

use std::path::PathBuf;

use herdr_harness::{Daemon, until};
use muster::proto::{OpenWindow, ReadDaemons, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::json;

#[test]
fn a_daemon_this_window_is_using_is_marked_and_the_rest_are_not() {
    let _turn = muster::testing::fresh_session();
    let attached = Daemon::start();
    let elsewhere = Daemon::start();
    attached.call("workspace.create", &json!({ "cwd": "/tmp", "label": "here", "focus": true }));

    // Written by hand rather than by starting a window's own daemon. What the window does when
    // it starts one is the contract tier's to prove, since only a real launch takes that path;
    // what this is about is the answer, and the answer reads the same record either way.
    let directory = scratch("census");
    muster_herdr::records::started(&directory, &attached.socket_path().to_string_lossy());
    muster_herdr::records::started(&directory, &elsewhere.socket_path().to_string_lossy());

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: attached.muster_config().to_string_lossy().into_owned(),
        daemon_records_path: directory.clone(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to be following the daemon it was pointed at",
        || !census().daemons.is_empty(),
        || "the census answered with nothing at all".to_string(),
    );

    let found = census();
    assert!(found.remembered, "a window told where to write records has a record to read");
    assert_eq!(found.daemons.len(), 2, "both daemons were written down: {found:?}");

    let here: Vec<&str> = found
        .daemons
        .iter()
        .filter(|daemon| daemon.attached_here)
        .map(|daemon| daemon.socket.as_str())
        .collect();
    assert_eq!(
        here,
        vec![attached.socket_path().to_string_lossy().as_ref()],
        "exactly the daemon this window is following is marked.\n  Impact: this is the field \
         the decision turns on. Marked wrong in one direction, somebody ends the daemon drawing \
         their own panes; wrong in the other, the census says every daemon on the machine is in \
         use and nothing is safe to end."
    );

    // And the daemon nothing is attached to is still a full row, because it is the row the
    // whole verb exists for: `muster window` already describes the one being used.
    let idle = found
        .daemons
        .iter()
        .find(|daemon| !daemon.attached_here)
        .expect("the second daemon is in the answer");
    assert_eq!(idle.state, "answering");
    assert_eq!(idle.panes, 0, "a daemon nothing has asked for a workspace holds nothing");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_window_with_nowhere_to_write_says_so_rather_than_answering_with_an_empty_machine() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    let found = census();
    assert!(
        !found.remembered,
        "a window with nowhere to write records has to say that is why the list is empty.\n  \
         Impact: 'Muster remembered nothing' and 'this machine has no daemons' are opposite \
         things to tell somebody about to reach for pkill, and an empty list alone reads as \
         the second."
    );
    assert!(found.daemons.is_empty());
}

fn census() -> muster::proto::Daemons {
    match answer(request::Payload::ReadDaemons(ReadDaemons {})).payload {
        Some(response::Payload::Daemons(daemons)) => daemons,
        other => panic!("a census answered with {other:?} rather than a list of daemons"),
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

fn scratch(name: &str) -> String {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path.to_string_lossy().into_owned()
}
