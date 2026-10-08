//! Hiding a pane and making one quiet, through the seam, against a real daemon.
//!
//! Both are the daemon's facts - a tab made hidden, a pane flagged quiet - and everything a window
//! does about them is decided on this side: which window holds a hidden tab (none), what the
//! roster lists it under, where a pane brought back lands, and what a quiet pane may ask. So this
//! sends what a shell or the CLI sends and reads what a shell renders.

use std::sync::Mutex;

use muster::proto::{
    AttentionChanged, ClosePane, Event, FocusPane, HidePane, OpenWindow, QuietPane, ReadWindow,
    Request, Response, RosterChanged, ShowPane, SplitPane, Startup, event, request, response,
};
use muster_daemon_proto::{Side, input_event};
use muster_harness::requests::{beside, create, in_new_tab, make, snapshot, until_text};
use muster_harness::{Daemon, Input, until};
use prost::Message;

/// Hiding takes a pane out of the window and into the hidden list, held by its daemon; showing
/// brings it back into a tab of its own with the keyboard on it.
#[test]
fn a_hidden_pane_leaves_the_window_and_comes_back_in_a_tab_of_its_own() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let keyboard_was = keyboard();

    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));
    until(
        "p2 to leave the window for the hidden list",
        || rows() == ["p1"] && hidden() == ["p2"],
        || format!("the window lists {:?} and hides {:?}", rows(), hidden()),
    );
    let mut control = daemon.connect();
    let held = snapshot(&mut control);
    let tab = held.tabs.iter().find(|tab| tab_holds(tab, "p2")).expect("p2 is in a tab");
    assert!(tab.hidden, "the daemon does not hold p2's tab as hidden: {tab:?}");
    assert_eq!(keyboard(), keyboard_was, "hiding another pane moved the keyboard");

    // Hiding it again changes nothing, and says so as a success.
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));

    assert_ok(&answer(request::Payload::ShowPane(ShowPane {
        pane_id: "p2".to_string(),
        take_focus: true,
        ..ShowPane::default()
    })));
    until(
        "p2 to come back in a tab of its own, with the keyboard",
        || {
            hidden().is_empty()
                && tabs() == 2
                && rows_in_tab_of("p2") == ["p2"]
                && keyboard().as_deref() == Some("p2")
        },
        || {
            format!(
                "the window lists {:?} in {} tabs, hides {:?}, and has the keyboard on {:?}",
                rows(),
                tabs(),
                hidden(),
                keyboard()
            )
        },
    );
}

/// Going to a hidden pane - a banner, the chord to whatever asks, a click on its row - brings it
/// back first, since nothing can show it where it is.
#[test]
fn focusing_a_hidden_pane_brings_it_back() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    assert_ok(&answer(request::Payload::HidePane(HidePane {
        pane_id: "p2".to_string(),
        ..HidePane::default()
    })));
    until("p2 to be hidden", || hidden() == ["p2"], || format!("hidden: {:?}", hidden()));

    assert_ok(&answer(request::Payload::FocusPane(FocusPane {
        pane_id: "p2".to_string(),
        ..FocusPane::default()
    })));
    until(
        "p2 to be back, with the keyboard",
        || hidden().is_empty() && keyboard().as_deref() == Some("p2"),
        || format!("hidden: {:?}, keyboard on {:?}", hidden(), keyboard()),
    );
}

/// A pane made hidden and quiet from the start is the headless agent: no window takes its tab,
/// the hidden list says it is quiet, and it closes by name though nothing shows it.
#[test]
fn a_pane_made_hidden_is_taken_by_no_window_and_closes_by_name() {
    let _turn = muster::testing::fresh_session();
    let daemon = two_panes_side_by_side();
    open_window(&daemon);
    let made = match answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p1".to_string(),
        side: "right".to_string(),
        hidden: true,
        quiet: true,
        ..SplitPane::default()
    }))
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("making a hidden pane answered {other:?}"),
    };
    until(
        "the new pane to be listed hidden and quiet",
        || hidden() == [made.clone()] && quiet_hidden() == [made.clone()],
        || format!("hidden: {:?}, quiet among them: {:?}", hidden(), quiet_hidden()),
    );
    assert_eq!(rows(), ["p1", "p2"], "the window's own tabs changed");

    assert_ok(&answer(request::Payload::ClosePane(ClosePane {
        pane_id: made.clone(),
        ..ClosePane::default()
    })));
    until("the hidden pane to close", || hidden().is_empty(), || format!("{:?}", hidden()));

    // Hidden and taking the keyboard is a contradiction, and is refused rather than half-done.
    let refused = answer(request::Payload::SplitPane(SplitPane {
        pane_id: "p1".to_string(),
        side: "right".to_string(),
        hidden: true,
        take_focus: true,
        ..SplitPane::default()
    }));
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "a hidden pane taking the keyboard answered {refused:?}"
    );
}

/// A quiet pane asks for nobody: a program's notification there raises nothing, where the same
/// notification in a pane that is not quiet does.
#[test]
fn a_quiet_pane_asks_for_nobody() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", in_new_tab("t2")));
    make(&mut control, create("p3", in_new_tab("t3")));
    until_text(&mut control, "p3", "$");
    open_window(&daemon);
    assert_ok(&answer(request::Payload::QuietPane(QuietPane {
        pane_id: "p2".to_string(),
        quiet: Some(true),
        ..QuietPane::default()
    })));
    until("p2 to be quiet", || quiet_rows() == ["p2"], || format!("{:?}", quiet_rows()));

    // Neither p2 nor p3 is the pane on screen, so each would ask if it could. p3 goes second, and
    // its asking is what says the notification from p2 has been and gone.
    let mut input = Input::connect(daemon.socket_path());
    for pane in ["p2", "p3"] {
        let send = input_event::Send {
            text: r"printf '\033]9;look here\a'".to_string(),
            enter: true,
            ..Default::default()
        };
        input.send(pane, input_event::Input::Send(send));
    }
    until(
        "p3's notification to ask",
        || asked().iter().any(|asked| asked.pane_id == "p3" && asked.state == "notified"),
        || format!("the window asked {:?}", asked()),
    );
    assert!(
        !asked().iter().any(|asked| asked.pane_id == "p2" && asked.state != "withdrawn"),
        "the quiet pane asked: {:?}",
        asked()
    );

    // Toggled, as a menu item sends it, it notifies again.
    assert_ok(&answer(request::Payload::QuietPane(QuietPane {
        pane_id: "p2".to_string(),
        ..QuietPane::default()
    })));
    until("p2 to notify again", || quiet_rows().is_empty(), || format!("{:?}", quiet_rows()));
}

fn two_panes_side_by_side() -> Daemon {
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", Side::Right)));
    daemon
}

fn open_window(daemon: &Daemon) {
    ROSTER.lock().expect("a panicking test poisoned the roster").take();
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to list its panes",
        || !rows().is_empty() && keyboard().is_some(),
        || format!("the roster holds {:?}", rows()),
    );
}

fn tab_holds(tab: &muster_daemon_proto::Tab, pane: &str) -> bool {
    fn holds(node: &muster_daemon_proto::Node, pane: &str) -> bool {
        match node.node.as_ref() {
            Some(muster_daemon_proto::node::Node::Pane(held)) => held == pane,
            Some(muster_daemon_proto::node::Node::Split(split)) => {
                split.first.as_ref().is_some_and(|first| holds(first, pane))
                    || split.second.as_ref().is_some_and(|second| holds(second, pane))
            }
            None => false,
        }
    }
    tab.root.as_ref().is_some_and(|root| holds(root, pane))
}

static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);
static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
        }
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        _ => {}
    }
}

fn roster() -> RosterChanged {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone().unwrap_or_default()
}

/// Every pane in the window's own tabs, in the order the roster lists them.
fn rows() -> Vec<String> {
    roster().tabs.iter().flat_map(|tab| &tab.panes).map(|pane| pane.pane_id.clone()).collect()
}

fn tabs() -> usize {
    roster().tabs.len()
}

fn rows_in_tab_of(pane: &str) -> Vec<String> {
    roster()
        .tabs
        .iter()
        .filter(|tab| tab.panes.iter().any(|held| held.pane_id == pane))
        .flat_map(|tab| &tab.panes)
        .map(|held| held.pane_id.clone())
        .collect()
}

/// Every pane the roster lists as hidden.
fn hidden() -> Vec<String> {
    roster().hidden.iter().flat_map(|tab| &tab.panes).map(|pane| pane.pane_id.clone()).collect()
}

fn quiet_hidden() -> Vec<String> {
    let roster = roster();
    roster
        .hidden
        .iter()
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.quiet)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn quiet_rows() -> Vec<String> {
    let roster = roster();
    roster
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .filter(|pane| pane.quiet)
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

/// Which pane the window's keyboard is on, read off the view.
fn keyboard() -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            let view = window.view?;
            let region =
                view.regions.iter().find(|region| region.region_id == view.focused_region)?;
            Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Opened(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
