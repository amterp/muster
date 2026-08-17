//! Dragging a row, through the seam, against a real daemon.
//!
//! The gesture the suite could describe from every angle and never perform. Its pieces are
//! each covered: `SidebarTests` decides which drops are legal, `backend-intent.json` pins the
//! envelope the adapter builds, and `pane_arranging.rs` drives herdr's own verbs and reads
//! the mirror. Between the protobuf a window sends and the roster a window draws there was
//! nothing, so nothing could be wrong about it - which is the shape of every bug this tier
//! was added for.
//!
//! So this sends the bytes a shell sends and reads the bytes a shell renders. `ArrangePane`
//! in, `RosterChanged` out, a real herdr behind it, and no daemon verb named anywhere in the
//! test: which of `pane.swap` and `pane.move` a drop becomes is the core's decision and is
//! exactly what would go unnoticed.
//!
//! One test per binary, for the reason the others here are: the seam holds one session per
//! process, and a `Startup` points the whole process at one daemon.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    ArrangePane, Event, OpenWindow, Request, Response, RosterChanged, Startup, event, request,
    response,
};
use prost::Message;
use serde_json::json;

#[test]
fn a_row_dropped_on_another_moves_the_pane_it_names() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
    daemon.call("pane.split", &json!({ "direction": "right" }));

    muster::ffi::muster_set_event_callback(Some(note_roster));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to list the two panes it opened onto",
        || rows().len() == 2,
        || format!("the roster holds {:?}", rows()),
    );
    let before = rows();
    let (first, second) = (before[0].clone(), before[1].clone());
    // Named rather than left empty. A drag knows which machine it happened on, and the seam
    // refuses a move that does not say - pane ids repeat across daemons, so "the focused
    // one" would land the drop on whichever `w1:p1` was found first.
    let machine = daemon_id();

    // The drop: the first row onto the second. Same tab, so the two exchange places - and
    // what makes this worth running is that the order comes back from the daemon's own tree
    // rather than from anything Muster arranged, so a request that reached the wrong pane
    // reads here as a list that did not move.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: machine.clone(),
        pane_id: first.clone(),
        onto_pane_id: second.clone(),
    })));

    until(
        "the two rows to exchange places",
        || rows() == vec![second.clone(), first.clone()],
        || format!("the roster holds {:?}, and started as {before:?}", rows()),
    );

    // Both panes still listed, once each. An exchange that lost one, or that grew a third
    // from an echo applied twice, would satisfy an assertion about the first row alone.
    let after = rows();
    let mut sorted = after.clone();
    sorted.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(sorted, expected, "the drag changed which panes exist, not only their order");

    // And back, because an exchange that works one way is one nobody can undo - and because
    // the second drop starts from the arrangement the first produced rather than from the one
    // the daemon opened with.
    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: machine,
        pane_id: first.clone(),
        onto_pane_id: second.clone(),
    })));
    until(
        "the rows to go back",
        || rows() == before,
        || format!("the roster holds {:?}, and should have returned to {before:?}", rows()),
    );
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note_roster(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::RosterChanged(roster)) = event.payload {
        *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
    }
}

/// Every pane the roster lists, in the order it lists them.
///
/// The list a person reads down, flattened across daemons and tabs the way the sidebar draws
/// it - which is the order `cmd+1` to `cmd+9` count in, so it is the answer the gesture is
/// about rather than a convenience.
fn rows() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.daemons)
        .flat_map(|daemon| &daemon.tabs)
        .flat_map(|tab| &tab.panes)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

/// The daemon the roster says these panes are on.
fn daemon_id() -> String {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.daemons)
        .map(|daemon| daemon.daemon_id.clone())
        .next()
        .expect("the roster names no daemon at all")
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
