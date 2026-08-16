//! What ⌘1 to ⌘9 mean, end to end against a real daemon.
//!
//! Every layer of this is already covered: `bindings.json` pins the nine chords, `roster.json`
//! pins the numbering they name, and `composition.json` pins what surfacing a tab does. What
//! is not covered anywhere else is the composition of those layers - which is the shape every
//! bug this project has shipped recently has had, each one green at every level and wrong when
//! the app ran.
//!
//! So this asserts the gesture: the number the roster hands the shell is the number the shell
//! can send back, and doing so lands the keyboard on the pane whose row carries it. The
//! interesting case is a pane in a tab nothing is showing, because that is the argument for
//! numbering panes at all - reaching one has to bring its tab on screen, or nine chords do not
//! replace what the tab numbers used to do.
//!
//! One test in this binary, on purpose. The seam holds the session in a process global and
//! this points the whole process at a scratch daemon through the environment; a second test
//! here would race both.

use std::sync::Mutex;

use herdr_harness::Daemon;
use muster::proto::{
    Event, FocusPaneAt, OpenWindow, Request, Response, RosterChanged, Startup, ViewChanged, event,
    request, response,
};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_numbered_chord_lands_on_the_row_carrying_that_number() {
    let daemon = Daemon::start();
    let (visible, hidden) = a_session_of_two_tabs(&daemon);

    // Registered before startup, because startup begins following the configured daemons and a
    // callback added afterwards misses the first bootstrap entirely.
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the roster to arrive with both tabs in it",
        || roster().is_some_and(|roster| places(&roster).len() == 2),
        || format!("the roster holds {:?}", roster().map(|r| places(&r))),
    );

    // The numbering is one count across every tab, so the pane in the second tab is 2 even
    // though it is the first pane of its own tab. Read off the roster rather than assumed: the
    // whole point is that the shell sends back the number it was given.
    let numbered = places(&roster().expect("just waited for it"));
    assert_eq!(
        numbered,
        vec![(1, visible.clone()), (2, hidden.clone())],
        "the count should run across tabs, so the second tab's first pane is 2"
    );

    // The tab holding pane 2 is not on screen: one region, showing the first tab. That is the
    // case the numbers exist for, and the one a tab-numbering scheme handled by numbering tabs.
    assert!(
        !showing(&hidden),
        "this test is pointless unless pane 2 starts hidden, and the view already shows it"
    );

    assert_ok(&answer(request::Payload::FocusPaneAt(FocusPaneAt { place: 2 })));

    until(
        "the hidden pane to be on screen with the keyboard",
        || showing(&hidden),
        || format!("the view still shows {:?}", shown()),
    );

    // Surfaced rather than opened beside: a second region onto the same daemon would be two
    // copies of one window, and the region count is what tells those apart.
    assert_eq!(regions(), 1, "reaching a hidden pane opened a region instead of retargeting one");

    // And the refusal, in the same breath, because a place past the end is what ⌘9 means in a
    // window of two and it has to do nothing rather than land somewhere.
    let reason = refusal(request::Payload::FocusPaneAt(FocusPaneAt { place: 9 }));
    assert!(
        reason.contains("2 panes") && reason.contains("no pane 9"),
        "a place past the end should say how many there are, and said: {reason}"
    );
    assert!(
        showing(&hidden),
        "a refused chord moved the keyboard, so it did something rather than nothing"
    );
}

/// Two tabs on one daemon, one pane each, so that the second pane is in a tab nothing shows.
///
/// Returns the pane in the tab a region will land on, then the pane behind it.
fn a_session_of_two_tabs(daemon: &Daemon) -> (String, String) {
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "numbering", "focus": true }));
    let first = only_pane(daemon);
    daemon.call("tab.create", &json!({ "focus": false }));
    let second = panes(daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the new tab brings a pane of its own");
    (first, second)
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace holds one pane, and held {panes:?}");
    panes.into_iter().next().expect("just counted one")
}

fn panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Every pane's place and id, in the order the roster lists them.
fn places(roster: &RosterChanged) -> Vec<(u32, String)> {
    roster
        .daemons
        .iter()
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| (pane.place, pane.pane_id.clone()))
        .collect()
}

/// Whether the view has this pane on screen with the keyboard on it.
fn showing(pane: &str) -> bool {
    shown().as_deref() == Some(pane)
}

fn shown() -> Option<String> {
    let view = VIEW.lock().expect("a panicking test poisoned the view");
    let view = view.as_ref()?;
    let focused = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    (!focused.pane_id.is_empty()).then(|| focused.pane_id.clone())
}

fn regions() -> usize {
    VIEW.lock()
        .expect("a panicking test poisoned the view")
        .as_ref()
        .map(|view| view.regions.len())
        .unwrap_or_default()
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
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

fn roster() -> Option<RosterChanged> {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone()
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

fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected the core to refuse this, and it answered {other:?}"),
    }
}

fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}. {}", on_failure());
}
