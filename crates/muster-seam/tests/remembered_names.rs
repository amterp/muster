//! Whether a pane and a tab keep the names they were given across a restart of the app.
//!
//! The claim that makes minted names usable at all, and it is a different claim for each noun.
//! A pane's name reaches it once, in the environment of the request that created it, and lives in
//! that process for as long as its shell does - which is longer than any one Muster. So a launch
//! that named every pane afresh would leave an agent that has been working since yesterday holding
//! a name nothing resolves, and every command it sent would be refused for a pane it is sitting in.
//! A tab's name is in nobody's environment, and is written down for the arrangement instead: the
//! saved window records which tab each region was showing, so a launch that named tabs afresh
//! would fail every region's check and open the window as a first launch, every launch.
//!
//! Driven from the file rather than by restarting the app, because the file is the whole
//! mechanism: written on publish, read before any daemon is attached. This writes one by hand
//! naming a pane and a tab the daemon already holds, and asserts the window comes up calling them
//! that - then that the next publish writes the file back with both names in it.
//!
//! One test here so far, and no longer because a second could not be had: the seam's session
//! is reset between tests and they take their turns through `muster::testing::fresh_session`,
//! which is what the first line of each one is asking for.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, Request, Response, RosterChanged, Startup, event, request, response,
};
use prost::Message;
use serde_json::json;

/// Names in the shape this Muster mints, so the test is about remembering rather than about
/// what the registry would accept.
const REMEMBERED: &str = "p1w3r07bsd";
const REMEMBERED_TAB: &str = "t1w3r07bsd";

#[test]
fn a_pane_and_a_tab_keep_the_names_they_had_before_this_launch() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    daemon
        .call("workspace.create", &json!({ "cwd": "/tmp", "label": "remembered", "focus": true }));
    let pane = only_pane(&daemon);
    let tab = only_tab(&daemon);

    // The file a previous Muster would have left behind, naming a pane and a tab this daemon still
    // holds. Written before startup because that is when it is read, and it is read then because
    // the first snapshot mints a name for everything it describes.
    let names_path = daemon.root().join("panes.toml");
    std::fs::write(
        &names_path,
        format!(
            "version = 1\n\n\
             [[pane]]\nname = \"{REMEMBERED}\"\ndaemon = \"local\"\nbackend = \"{pane}\"\n\n\
             [[tab]]\nname = \"{REMEMBERED_TAB}\"\ndaemon = \"local\"\nbackend = \"{tab}\"\n"
        ),
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

    assert_eq!(
        tabs_listed(),
        vec![REMEMBERED_TAB.to_string()],
        "the window named this tab afresh instead of keeping what the file said.\n  Impact: the \
         saved arrangement names the tab each region was showing, so none of them resolve and the \
         window opens as a first launch - every launch, not just this one.\n  Check that the tab \
         registry is read back in set_pane_names_path beside the pane one."
    );

    // And written back. A launch that read the file and then replaced it with names of its own
    // would pass everything above and strand the pane at the *next* restart instead.
    let read_back = || std::fs::read_to_string(&names_path).unwrap_or_default();
    until(
        "the file to be written back with both names still in it",
        || read_back().contains(REMEMBERED) && read_back().contains(REMEMBERED_TAB),
        || format!("the file holds: {}", read_back()),
    );
}

fn only_tab(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let tabs =
        snapshot["snapshot"]["tabs"].as_array().unwrap_or_else(|| panic!("no tabs in {snapshot}"));
    assert_eq!(tabs.len(), 1, "a fresh workspace holds one tab, and held {tabs:?}");
    tabs[0]["tab_id"].as_str().expect("a tab carries an id").to_string()
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

/// Every tab the window lists, by the name Muster calls it.
fn tabs_listed() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking reader poisoned the roster")
        .as_ref()
        .into_iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| daemon.tabs.iter())
        .map(|tab| tab.tab_id.clone())
        .collect()
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
