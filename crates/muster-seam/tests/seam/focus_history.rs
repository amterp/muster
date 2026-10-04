//! Walking back and forward through the panes the keyboard has been on, against a real daemon.
//!
//! What the mouse's back and forward buttons do, and `focus_back`, `focus_forward` and `muster
//! focus --back` with them. The rules themselves are `muster_core::focus_history`'s to pin; what
//! needs a window is that every way the keyboard moves is recorded, that a step back brings the
//! pane's tab on screen, and that a pane which closed is not somewhere to go back to.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use muster::proto::{
    ClosePane, CreateTab, Event, FocusHistory, MoveTab, OpenWindow, Request, Response,
    RosterChanged, SplitPane, Startup, ViewChanged, Went, event, request, response,
};
use muster_core::composition::holding::to_toml;
use muster_core::composition::{DaemonId, HeldWindow, Holders, WindowName};
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

    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
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
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
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

/// A pane whose tab a closed window holds now is stepped over, rather than gone to by asking for
/// that window back: back means somewhere this window can go.
#[test]
fn a_pane_a_closed_window_holds_now_is_stepped_over() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let path = record(&daemon);
    let mut holders = Holders::new();
    let closed = a_window_named(&daemon, "window-9");
    let name = closed.name.clone();
    holders.opened(closed);
    holders.closed(&name);
    write_record(&path, &holders);
    open_onto(&daemon);
    let (first, middle, _) = three_tabs();
    let given = tab_of(&middle).expect("the middle pane is in a tab");

    assert_ok(&answer(request::Payload::MoveTab(MoveTab {
        tab_id: given,
        window: name.to_string(),
    })));
    until(
        "the tab given to the closed window to leave this one",
        || !panes().contains(&middle),
        || format!("this window still holds {:?}", panes()),
    );
    let asked = REOPENS.load(Ordering::SeqCst);

    assert_eq!(walk(false).pane_id, first, "back did not step over the closed window's pane");
    assert_eq!(keyboard().as_deref(), Some(first.as_str()));
    assert_eq!(
        REOPENS.load(Ordering::SeqCst),
        asked,
        "going back asked for a closed window to be opened again"
    );
}

/// A daemon, and a window open onto its one pane with the keyboard on it.
fn a_window() -> Daemon {
    let daemon = Daemon::start_built();
    open_onto(&daemon);
    daemon
}

/// A window open onto a daemon's one pane with the keyboard on it.
fn open_onto(daemon: &Daemon) {
    // The last test's window, which would otherwise answer for this one until it publishes.
    *ROSTER.lock().expect("a panicking test poisoned the roster") = None;
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to open onto a pane with the keyboard on it",
        || keyboard().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
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

/// Three tabs, each with the keyboard on its one pane in turn, and the three panes in that order.
fn three_tabs() -> (String, String, String) {
    let first = keyboard().expect("the window opened with the keyboard on a pane");
    let mut made = Vec::new();
    for _ in 0..2 {
        assert_ok(&answer(request::Payload::CreateTab(CreateTab {
            take_focus: true,
            ..CreateTab::default()
        })));
        let before = made.last().cloned().unwrap_or_else(|| first.clone());
        until(
            "the new tab to take the keyboard",
            || keyboard().is_some_and(|pane| pane != before),
            || format!("the keyboard is on {:?}", keyboard()),
        );
        made.push(keyboard().expect("just waited for it"));
    }
    let last = made.pop().expect("two tabs were made");
    let middle = made.pop().expect("two tabs were made");
    (first, middle, last)
}

fn record(daemon: &Daemon) -> PathBuf {
    daemon.root().join("holding/tabs.toml")
}

fn write_record(path: &Path, holders: &Holders) {
    std::fs::create_dir_all(path.parent().expect("the record is in a directory"))
        .expect("the record's directory can be made");
    std::fs::write(path, to_toml(holders)).expect("the record can be written");
}

/// A row for another window, with an arrangement that exists so the record keeps it.
fn a_window_named(daemon: &Daemon, name: &str) -> HeldWindow {
    let arrangement = daemon.root().join(format!("{name}.toml"));
    std::fs::write(&arrangement, "").expect("a stand-in arrangement can be written");
    HeldWindow {
        name: WindowName::new(name),
        arrangement: arrangement.to_string_lossy().into_owned(),
        socket: String::new(),
        pid: 0,
        install: String::new(),
        focused: 0,
        daemons: std::iter::once(DaemonId::new("local")).collect(),
    }
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);
/// How many times the core has asked for a closed window to be opened again.
static REOPENS: AtomicUsize = AtomicUsize::new(0);
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
        Some(event::Payload::ReopenWindow(_)) => {
            REOPENS.fetch_add(1, Ordering::SeqCst);
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
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
