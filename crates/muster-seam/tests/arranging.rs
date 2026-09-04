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
//! Both destinations a move has are here, because both are decided on this side of the seam:
//! whether a drop becomes a swap or a move is read off where the two panes are, and whether the
//! tab a move makes comes on screen is the window's own answer rather than the daemon's.

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
    let _turn = muster::testing::fresh_session();
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
        ..ArrangePane::default()
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
        ..ArrangePane::default()
    })));
    until(
        "the rows to go back",
        || rows() == before,
        || format!("the roster holds {:?}, and should have returned to {before:?}", rows()),
    );
}

/// Pulling a pane out of a split is one request, and it costs no pane.
///
/// The dance this replaces made a tab, moved into it, and closed the pane the first command had
/// started - a login shell on this machine and an ssh session on another, opened and killed
/// seconds apart, with the keyboard passing through it on the way (kan `a_2IXGSgZi7`).
///
/// Counting the panes before and afterwards is what says so. A tab holding one pane is the same
/// picture whether nothing extra was made or something was made and thrown away, and only the
/// count tells the two apart.
#[test]
fn a_pane_pulled_into_a_tab_of_its_own_costs_no_pane_and_no_keyboard() {
    let _turn = muster::testing::fresh_session();
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
        || rows().len() == 2 && tabs().len() == 1,
        || format!("the roster holds {:?} in {:?}", rows(), tabs()),
    );
    let before = rows();
    let keyboard_was = keyboard();
    // A pane the keyboard is not on, which is the case worth pinning: an agent pulling another
    // agent's pane out of a split must not lose its own place doing it. Pulling the pane the
    // keyboard is on moves it either way, because the pane it was on has left the region.
    let pulled = before
        .iter()
        .find(|pane| Some((*pane).clone()) != keyboard_was)
        .expect("the window opened onto more than one pane")
        .clone();

    assert_ok(&answer(request::Payload::ArrangePane(ArrangePane {
        daemon_id: daemon_id(),
        pane_id: pulled.clone(),
        new_tab: true,
        tab_name: "pulled out".to_string(),
        ..ArrangePane::default()
    })));

    until(
        "the pane to be alone in a tab of its own",
        || tabs().len() == 2 && rows_in_tab_of(&pulled) == vec![pulled.clone()],
        || format!("the roster holds {:?} in {:?}", rows(), tabs()),
    );

    let mut after = rows();
    after.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(
        after, expected,
        "the move changed which panes exist. A pane made and closed on the way is what this \
         command exists to stop costing."
    );
    assert_eq!(
        keyboard(),
        keyboard_was,
        "pulling somebody else's pane out of a split moved the keyboard, which is what the \
         dance this replaces did by way of the throwaway pane it opened"
    );
    // And the window did not change what it is showing. Bringing the new tab on screen would
    // put the tab somebody is working in behind it, which is the same interruption arriving a
    // different way.
    assert!(
        on_screen().contains(&keyboard_was.clone().expect("the keyboard is on a pane")),
        "the tab the keyboard is in went off screen, so the window followed the tab the move \
         made"
    );
    assert!(
        tabs().iter().any(|(_, label)| label.contains("pulled out")),
        "the new tab did not take the name the move gave it: {:?}",
        tabs()
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
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.panes)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

/// Every tab the roster lists, with what it is called.
fn tabs() -> Vec<(String, String)> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .map(|tab| (tab.tab_id.clone(), tab.label.clone()))
        .collect()
}

/// The panes sharing a tab with this one, in the order the roster lists them.
fn rows_in_tab_of(pane: &str) -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .filter(|tab| tab.panes.iter().any(|held| held.pane_id == pane))
        .flat_map(|tab| &tab.panes)
        .map(|held| held.pane_id.clone())
        .collect()
}

/// Every pane the window says it is drawing.
fn on_screen() -> Vec<String> {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.on_screen)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

/// Which pane the window's keyboard is on, as the roster's own marking has no answer for.
///
/// Read off the view rather than the roster: the roster says what exists and the view says
/// where the keyboard is, and this test is about the second.
fn keyboard() -> Option<String> {
    match answer(request::Payload::ReadWindow(muster::proto::ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => {
            let view = window.view?;
            let region =
                view.regions.iter().find(|region| region.region_id == view.focused_region)?;
            Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The daemon the roster says these panes are on.
fn daemon_id() -> String {
    ROSTER
        .lock()
        .expect("a panicking test poisoned the roster")
        .iter()
        .flat_map(|roster| &roster.tabs)
        .flat_map(|tab| &tab.daemon_ids)
        .next()
        .cloned()
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
