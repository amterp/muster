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
//! The daemon keeps both: it knows a pane and a tab by the name the Muster that made them minted,
//! and says so in every snapshot. So this makes a pane and a tab on the daemon under names in the
//! shape a previous Muster would have minted, and asserts a window opening onto it calls them
//! that rather than naming them again.

use std::sync::Mutex;

use muster::proto::{
    Event, OpenWindow, Request, Response, RosterChanged, Startup, event, request, response,
};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// Names in the shape this Muster mints, so the test is about remembering rather than about
/// what the window would accept.
const REMEMBERED: &str = "p1w3r07bsd";
const REMEMBERED_TAB: &str = "t1w3r07bsd";

#[test]
fn a_pane_and_a_tab_keep_the_names_they_had_before_this_launch() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    // What a previous Muster left behind: a pane and a tab it named, still running.
    make(&mut daemon.connect(), create(REMEMBERED, in_new_tab(REMEMBERED_TAB)));

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until("the roster to arrive", || !listed().is_empty(), || "nothing was listed".to_string());
    assert_eq!(
        listed(),
        vec![REMEMBERED.to_string()],
        "the window named this pane afresh instead of keeping the name the daemon holds it \
         by.\n  Impact: a program running in it since before this launch holds a name that \
         resolves to nothing, so every command from inside it is refused."
    );

    assert_eq!(
        tabs_listed(),
        vec![REMEMBERED_TAB.to_string()],
        "the window named this tab afresh instead of keeping the name the daemon holds it \
         by.\n  Impact: the saved arrangement names the tab each region was showing, so none of \
         them resolve and the window opens as a first launch - every launch, not just this one."
    );
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    if let Ok(Event { payload: Some(event::Payload::RosterChanged(roster)), .. }) =
        Event::decode(bytes)
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
        .flat_map(|roster| roster.tabs.iter())
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
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| pane.pane_id.clone())
        .collect()
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
