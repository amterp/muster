//! Opening onto a daemon somebody else has been using.
//!
//! Every other daemon-backed test here builds its own world first, so the daemon Muster meets
//! is one Muster arranged - one workspace, one tab, panes it asked for. That is the least
//! likely state a real daemon is in. Sessions outlive the app by design, so the ordinary case
//! is a daemon with somebody's work already in it: a workspace Muster did not create, several
//! tabs, one of them zoomed, and a focused tab chosen by a person rather than by this window.
//!
//! Arranged through herdr's own API rather than through the path under test, so a broken open
//! fails at the assertion and not at the setup.
//!
//! Invariants over whatever the window settles on, not a sequence. What a view *should* be
//! here depends on which tab a daemon happened to be focused on, and a test that pinned one
//! answer would be a test about the arrangement rather than about the rules. The rules are:
//! every pane on screen can be typed into, every pane that exists can be found, and the
//! keyboard is somewhere.
//!
//! One state deliberately not covered, because it does not exist: a pane whose terminal has
//! died. herdr drops such a pane from `pane.list` the moment its process goes, so what a
//! daemon can hold is a live pane or no pane. A pane that dies while Muster is attached is a
//! different case and has its own coverage - it is a pane the daemon stops holding.
//!
//! Its own binary because the seam holds one session per process.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, FocusPane, OpenWindow, Request, Response, RosterChanged, Startup, ViewChanged, ViewNode,
    event, request, response, view_node,
};
use prost::Message;
use serde_json::json;

#[test]
fn a_window_opened_on_somebody_elses_session_can_reach_all_of_it() {
    let daemon = Daemon::start();

    // Somebody else's work, made before Muster has heard of this daemon. A second tab holding
    // two panes with one zoomed is the shape that has bitten twice: a zoomed tab publishes
    // only the zoomed pane, and a tab nobody is showing is where a finished agent hides.
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "theirs", "focus": true }));
    daemon.call("tab.create", &json!({ "workspace_id": "w1", "cwd": "/tmp" }));
    let hidden = panes_in_second_tab(&daemon);
    daemon.call(
        "pane.split",
        &json!({ "target_pane_id": hidden, "direction": "down", "cwd": "/tmp" }),
    );
    daemon.call("pane.zoom", &json!({ "pane_id": hidden }));

    // The world, before Muster sees any of it. Asserted rather than assumed, because every
    // check below passes vacuously on a daemon holding one pane - which is what a `tab.create`
    // that quietly failed would leave, and is exactly the shape this test exists to avoid.
    let held = daemon_panes(&daemon);
    assert_eq!(
        held.len(),
        3,
        "the arrangement did not take, so what follows would be a test about one pane: {held:?}"
    );

    // Given a name each, because Muster mints its own name for every pane and nothing here can
    // predict it - a given name is the one handle both sides hold. Before startup, since herdr
    // announces a rename to nobody and the bootstrap snapshot is the only thing carrying one.
    let given: Vec<String> = (1..=held.len()).map(|nth| format!("theirs-{nth}")).collect();
    for (pane, name) in held.iter().zip(&given) {
        daemon.call("pane.rename", &json!({ "pane_id": pane, "label": name }));
    }

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to show something of this session",
        || latest_view().is_some_and(|view| !view_panes(&view).is_empty()),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // Typeable, for every pane on screen. A pane published without a control socket is one the
    // shell must not spawn a bridge for, so it renders blank and nothing republishes it.
    let view = latest_view().expect("just waited for it");
    for (region, pane, socket) in view_panes(&view) {
        assert!(
            !socket.is_empty(),
            "region {region} shows {pane} with no control socket, so it would paint nothing \
             and swallow the keyboard"
        );
    }
    assert!(
        view.regions.iter().any(|region| !region.pane_id.is_empty()),
        "no region has the keyboard, so the first thing typed goes nowhere: {view:?}"
    );

    // Findable, for every pane that exists. This is the founding desideratum and the half a
    // view cannot carry: the pane most likely to have finished unnoticed is the one no region
    // is showing, and here two of the four are in a tab nothing opened onto.
    let roster = latest_roster().expect("a window that published a view published a roster");
    let listed: Vec<String> = roster_panes(&roster).map(|pane| pane.pane_id.clone()).collect();
    let by_given: Vec<String> = roster_panes(&roster).map(|pane| pane.given_name.clone()).collect();
    for name in &given {
        assert!(
            by_given.contains(name),
            "the daemon holds a pane called {name} and the roster does not list it, so nothing \
             in this window can reach it: {by_given:?}"
        );
    }
    let mut sorted = listed.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), listed.len(), "the roster lists a pane twice: {listed:?}");

    // A zoomed tab publishes only its zoomed pane, so a roster built from what is on screen
    // would lose the other one. This is the assertion that says the roster is not that.
    assert!(
        listed.len() >= held.len(),
        "the roster is smaller than what the daemon holds ({} against {}), which is what a \
         roster derived from the view rather than from the session looks like",
        listed.len(),
        held.len()
    );

    // And reachable. Asking for a pane in the tab nothing is showing has to surface it, which
    // is what makes the roster a list of destinations rather than a report.
    let (daemon_id, pane_id) = roster_panes(&roster)
        .find(|pane| !pane.on_screen)
        .map(|pane| (pane.daemon_id.clone(), pane.pane_id.clone()))
        .expect(
            "every pane this daemon holds is already on screen, so the half of this test about \
             reaching a hidden one never ran - which is what a roster reporting only the view \
             would look like",
        );
    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        daemon_id,
        pane_id: pane_id.clone(),
    })));
    until(
        "the window to surface the pane nothing was showing",
        || {
            latest_view()
                .is_some_and(|view| view_panes(&view).iter().any(|(_, pane, _)| *pane == pane_id))
        },
        || {
            format!(
                "{pane_id} was asked for and no region shows it: {:?}",
                latest_view().map(|view| view_panes(&view))
            )
        },
    );
}

/// The pane the second tab was created around.
///
/// Read back rather than assumed: pane ids are the daemon's to hand out, and a test that
/// spelled one would be asserting herdr's numbering rather than Muster's behavior.
fn panes_in_second_tab(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes = snapshot["snapshot"]["panes"].as_array().expect("a session lists its panes");
    panes
        .iter()
        .find(|pane| pane["tab_id"].as_str() == Some("w1:t2"))
        .and_then(|pane| pane["pane_id"].as_str())
        .expect("the tab that was just created holds a pane")
        .to_string()
}

fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["panes"]
        .as_array()
        .map(|panes| {
            panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default()
}

/// Every pane the view names, with the region showing it and the socket a bridge would dial.
fn view_panes(view: &ViewChanged) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    for region in &view.regions {
        if let Some(root) = &region.root {
            collect(root, &region.region_id, &mut found);
        }
    }
    found
}

fn collect(node: &ViewNode, region: &str, into: &mut Vec<(String, String, String)>) {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            into.push((region.to_string(), pane.pane_id.clone(), pane.control_socket_path.clone()));
        }
        Some(view_node::Node::Split(split)) => {
            for child in [&split.first, &split.second].into_iter().flatten() {
                collect(child, region, into);
            }
        }
        None => {}
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn latest_roster() -> Option<RosterChanged> {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone()
}

/// Every pane the roster lists, flattened out of the daemon-tab-pane nesting.
fn roster_panes(roster: &RosterChanged) -> impl Iterator<Item = &muster::proto::RosterPane> {
    roster.daemons.iter().flat_map(|daemon| daemon.tabs.iter()).flat_map(|tab| tab.panes.iter())
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
        Some(_) => {}
    }
}
