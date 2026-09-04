//! Reaching a pane in a tab nothing is showing, against a real daemon.
//!
//! An agent told to "split two panes below you" has to be able to do it from a tab nobody is
//! looking at, and the window it is driving is very often not the window somebody is using. The
//! failure this covers cost two attempts and a `muster focus` to work around, both times while
//! setting up a parallel round - and the workaround takes the keyboard off whatever a person was
//! doing in another tab, to perform an act that has nothing to do with what is on screen
//! (kan `a_2A9TxffR8`).
//!
//! The rule underneath: where a new pane goes is a fact about the tab's tree. Closing is here
//! for the opposite reason - it destroys something, so it keeps asking the window which region
//! holds the thing, and the test says so rather than leaving the difference to be discovered.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    CloseTab, CreateTab, Event, OpenWindow, Request, Response, RosterChanged, SplitPane, Startup,
    ViewChanged, event, request, response,
};
use prost::Message;

#[test]
fn a_pane_in_a_tab_nothing_is_showing_can_be_split() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a workspace",
        || panes_of_tabs().len() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );
    let backgrounded = panes_of_tabs()[0].clone();

    // A second tab, which comes on screen and puts the first one behind it. That is the
    // arrangement the failure happens in and it is an ordinary one: a person made a tab, and
    // the agent's own pane is in the tab they left.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the new tab to come on screen and the first one to go behind it",
        || tabs().len() == 2 && tabs().iter().filter(|(_, on_screen)| *on_screen).count() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );
    let keyboard_was = keyboard();

    // Named rather than left to the keyboard, which is what an agent driving a window does:
    // it says which pane, and that pane is very often not the one somebody is typing into.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        pane_id: backgrounded.1.clone(),
        side: "down".to_string(),
        ..SplitPane::default()
    })));
    until(
        "the background tab to hold the pane the split made",
        || panes_in(&backgrounded.0) == 2,
        || format!("the last roster the core published: {:?}", panes_of_tabs()),
    );

    // And the keyboard is where it was. A split in a tab nothing is showing has no region to
    // move it into, and moving it would take somebody off what they were doing in the tab that
    // is on screen - which is the cost of the workaround this replaces.
    assert_eq!(
        keyboard(),
        keyboard_was,
        "splitting a pane in a background tab moved the keyboard out of the tab on screen"
    );
}

/// That the pane a split names has to exist, which the change above must not have loosened.
///
/// The refusal that guards it is a different one - the daemon does not hold that pane - and it
/// has to keep arriving, because the two failures look identical from a script: nothing happened
/// and nothing said why.
#[test]
fn a_split_still_refuses_a_pane_no_daemon_holds() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a workspace",
        || panes_of_tabs().len() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );

    let response = answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p0000000000".to_string(),
        side: "down".to_string(),
        ..SplitPane::default()
    }));
    match response.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            !failure.reason.is_empty(),
            "a refusal with nothing in it tells a script only that nothing happened"
        ),
        other => panic!("splitting a pane no daemon holds answered {other:?}"),
    }
}

/// Closing a tab ends every pane in it, whichever tab is on screen.
///
/// The verb that was missing: emptying a tab one pane at a time was the only route to a thing
/// people do (kan `a_2Ic6mB36E`). What it required was a region *showing* the tab, and that
/// stopped being a usable rule when a window came to show one tab at a time (MIP-2): every tab
/// but one would be unclosable, and `muster tab close --tab <t>` names one of those by design.
///
/// The guard that remains is that this window holds the tab, which is what it was protecting
/// against - a tab in a session nobody here is attached to. The property the old rule was really
/// after survives elsewhere: the agent list names every tab, so you can still see what you are
/// destroying.
#[test]
fn closing_a_tab_ends_it_whether_or_not_it_is_the_one_on_screen() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a workspace",
        || panes_of_tabs().len() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );
    let first = panes_of_tabs()[0].0.clone();

    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the new tab to come on screen and the first one to go behind it",
        || tabs().len() == 2 && tabs().iter().filter(|(_, on_screen)| *on_screen).count() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );

    let second = tabs()
        .into_iter()
        .find(|(tab, _)| tab != &first)
        .map(|(tab, _)| tab)
        .expect("the window holds two tabs");

    // A tab nobody here holds. Refused, and the refusal says this window is not showing it
    // rather than that the tab is gone.
    let refusal = answer(request::Payload::CloseTab(CloseTab {
        tab_id: "t0nesuch".to_string(),
        ..CloseTab::default()
    }));
    match refusal.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            !failure.reason.is_empty(),
            "a refusal with nothing in it tells a script only that nothing happened"
        ),
        other => panic!("closing a tab this window does not hold answered {other:?}"),
    }
    assert_eq!(tabs().len(), 2, "the refused close took a tab anyway");

    // The tab behind the one on screen, named outright. This is the case the old rule refused
    // and the one `muster tab close --tab` exists for.
    assert_ok(&answer(request::Payload::CloseTab(CloseTab {
        tab_id: first.clone(),
        ..CloseTab::default()
    })));
    until(
        "the background tab to be gone",
        || tabs().len() == 1,
        || format!("the last roster the core published: {:?}", tabs()),
    );
    assert_eq!(tabs()[0].0, second, "the wrong tab closed");

    // And the tab on screen goes when none is named, which is what the menu item means - the
    // tab the keyboard is in.
    assert_ok(&answer(request::Payload::CloseTab(CloseTab::default())));
    until(
        "the tab on screen to be gone too",
        || tabs().is_empty(),
        || format!("the last roster the core published: {:?}", tabs()),
    );
}

/// Every tab the window holds, and whether a region is showing it.
fn tabs() -> Vec<(String, bool)> {
    latest_roster()
        .map(|roster| roster.tabs.iter().map(|tab| (tab.tab_id.clone(), tab.on_screen)).collect())
        .unwrap_or_default()
}

/// One (tab, pane) pair per pane the window knows about.
fn panes_of_tabs() -> Vec<(String, String)> {
    latest_roster()
        .map(|roster| {
            roster
                .tabs
                .iter()
                .flat_map(|tab| {
                    tab.panes.iter().map(move |pane| (tab.tab_id.clone(), pane.pane_id.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn panes_in(tab: &str) -> usize {
    panes_of_tabs().iter().filter(|(held, _)| held == tab).count()
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
