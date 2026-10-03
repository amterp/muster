//! Walking back and forward through the panes the keyboard has been on, against a real daemon.
//!
//! What the mouse's back and forward buttons do, and `focus_back`, `focus_forward` and `muster
//! focus --back` with them. The rules themselves are `muster_core::focus_history`'s to pin; what
//! needs a window is that every way the keyboard moves is recorded, that a step back brings the
//! pane's tab on screen, and that a pane which closed is not somewhere to go back to.

use std::sync::Mutex;

use muster::proto::{
    ClosePane, CreateTab, Event, FocusHistory, OpenWindow, Request, Response, RosterChanged,
    SplitPane, Startup, ViewChanged, Went, event, request, response,
};
use muster_harness::{Daemon, until};
use prost::Message;

/// A new tab takes the keyboard, and back takes it to the pane in the tab before - bringing
/// that tab on screen, as a browser's back button brings back the page.
#[test]
fn back_goes_to_the_pane_in_the_tab_before_and_forward_returns() {
    let _turn = muster::testing::fresh_session();
    let _daemon = a_window();
    let first = keyboard().expect("the window opened with the keyboard on a pane");

    assert_eq!(walk(false), Went::default(), "a window's first pane has nothing before it");

    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    let second = keyboard().expect("the new tab took the keyboard");
    assert_ne!(second, first);

    assert_eq!(walk(false).pane_id, first);
    assert_eq!(keyboard().as_deref(), Some(first.as_str()));
    assert_eq!(on_screen(), tab_of(&first), "going back left the pane's tab behind");

    assert_eq!(walk(true).pane_id, second);
    assert_eq!(keyboard().as_deref(), Some(second.as_str()));
    assert_eq!(walk(true), Went::default(), "the newest pane has nothing after it");
}

/// Going back and then somewhere new is a fork, and the panes that were ahead are gone from
/// the history, as they are from a browser's.
#[test]
fn going_somewhere_new_after_going_back_drops_what_was_ahead() {
    let _turn = muster::testing::fresh_session();
    let _daemon = a_window();
    let first = keyboard().expect("the window opened with the keyboard on a pane");
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    assert_eq!(walk(false).pane_id, first);

    let made = split();
    assert_eq!(keyboard().as_deref(), Some(made.as_str()), "the split did not take the keyboard");

    assert_eq!(walk(true), Went::default(), "the tab gone back from is still ahead");
    assert_eq!(keyboard().as_deref(), Some(made.as_str()));
    assert_eq!(walk(false).pane_id, first);
}

#[test]
fn a_pane_that_closed_is_stepped_over() {
    let _turn = muster::testing::fresh_session();
    let _daemon = a_window();
    let first = keyboard().expect("the window opened with the keyboard on a pane");
    let middle = split();
    let last = split();

    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        pane_id: middle.clone(),
        ..ClosePane::default()
    })));
    until(
        "the closed pane to leave the window",
        || !panes().contains(&middle),
        || format!("the window still holds {:?}", panes()),
    );
    assert_eq!(keyboard().as_deref(), Some(last.as_str()));

    assert_eq!(walk(false).pane_id, first, "back went somewhere other than the pane before");
}

/// A daemon, and a window open onto its one pane with the keyboard on it.
fn a_window() -> Daemon {
    let daemon = Daemon::start_built();
    // The last test's window, which would otherwise answer for this one until it publishes.
    *ROSTER.lock().expect("a panicking test poisoned the roster") = None;
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a pane with the keyboard on it",
        || keyboard().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    daemon
}

/// Splits the pane the keyboard is on, moves the keyboard into the new one, and names it.
fn split() -> String {
    let response = answer(request::Payload::SplitPane(SplitPane {
        side: "right".to_string(),
        take_focus: true,
        ..SplitPane::default()
    }));
    match response.payload {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("a split answered {other:?}"),
    }
}

fn walk(forward: bool) -> Went {
    match answer(request::Payload::FocusHistory(FocusHistory { forward })).payload {
        Some(response::Payload::Went(went)) => went,
        other => panic!("the core answered a FocusHistory with {other:?}"),
    }
}

fn panes() -> Vec<String> {
    latest_roster()
        .map(|roster| {
            roster
                .tabs
                .iter()
                .flat_map(|tab| tab.panes.iter().map(|pane| pane.pane_id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn tab_of(pane: &str) -> Option<String> {
    latest_roster()?
        .tabs
        .into_iter()
        .find(|tab| tab.panes.iter().any(|held| held.pane_id == pane))
        .map(|tab| tab.tab_id)
}

fn on_screen() -> Option<String> {
    latest_roster()?.tabs.into_iter().find(|tab| tab.on_screen).map(|tab| tab.tab_id)
}

/// Which pane the window's keyboard is on, as the view says it.
fn keyboard() -> Option<String> {
    let view = latest_view()?;
    let region = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);
static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
        }
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        _ => {}
    }
}

fn latest_roster() -> Option<RosterChanged> {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone()
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
