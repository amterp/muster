//! Whether a pane keeps its name across a restart of the app.
//!
//! The claim that makes minted names usable at all. A pane's name reaches it once, in the
//! environment of the request that created it, and lives in that process for as long as its
//! shell does - which is longer than any one Muster. So a launch that named every pane afresh
//! would leave an agent that has been working since yesterday holding a name nothing resolves,
//! and every command it sent would be refused for a pane it is sitting in.
//!
//! Driven from the file rather than by restarting the app, because the file is the whole
//! mechanism: written on publish, read before any daemon is attached. This writes one by hand
//! naming a pane the daemon already holds, and asserts the window comes up calling it that -
//! then that the next publish writes the file back with the same name in it.
//!
//! One test in this binary, on purpose: the seam holds one session per process.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, Request, Response, RosterChanged, Startup, event, request, response,
};
use prost::Message;
use serde_json::json;

/// A name in the shape this Muster mints, so the test is about remembering rather than about
/// what the registry would accept.
const REMEMBERED: &str = "p1w3r07bsd";

#[test]
fn a_pane_keeps_the_name_it_was_given_before_this_launch() {
    let daemon = Daemon::start();
    daemon
        .call("workspace.create", &json!({ "cwd": "/tmp", "label": "remembered", "focus": true }));
    let pane = only_pane(&daemon);

    // The file a previous Muster would have left behind, naming a pane this daemon still holds.
    // Written before startup because that is when it is read, and it is read then because the
    // first snapshot mints a name for everything it describes.
    let names_path = daemon.root().join("panes.toml");
    std::fs::write(
        &names_path,
        format!("version = 1\n\n[[pane]]\nname = \"{REMEMBERED}\"\ndaemon = \"local\"\nbackend = \"{pane}\"\n"),
    )
    .expect("the harness root is writable");

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        pane_names_path: names_path.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until("the roster to arrive", || !listed().is_empty(), || "nothing was listed".to_string());
    assert_eq!(
        listed(),
        vec![REMEMBERED.to_string()],
        "the window named this pane afresh instead of keeping what the file said.\n  Impact: a \
         program running in it since before this launch holds a name that resolves to nothing, \
         so every command from inside it is refused.\n  Check that set_pane_names_path runs \
         before the config is applied - reading it afterwards is too late, because attaching \
         mints a name for every pane a snapshot describes."
    );

    // And written back. A launch that read the file and then replaced it with names of its own
    // would pass everything above and strand the pane at the *next* restart instead.
    let read_back = || std::fs::read_to_string(&names_path).unwrap_or_default();
    until(
        "the file to be written back with the name still in it",
        || read_back().contains(REMEMBERED),
        || format!("the file holds: {}", read_back()),
    );
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes = snapshot["snapshot"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("no panes in {snapshot}"));
    assert_eq!(panes.len(), 1, "a fresh workspace holds one pane, and held {panes:?}");
    panes[0]["pane_id"].as_str().expect("a pane carries an id").to_string()
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    if let Ok(Event { payload: Some(event::Payload::RosterChanged(roster)) }) = Event::decode(bytes)
    {
        *ROSTER.lock().expect("a panicking reader poisoned the roster") = Some(roster);
    }
}

/// Every pane the window lists, by the name Muster calls it.
fn listed() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| pane.pane_id.clone())
        .collect()
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
