//! Several windows in one process (MIP-6).
//!
//! What a window is to the core: a name the events it is sent carry, a target a request can name,
//! and a share of the tabs. Every test here holds two real windows in one session against a real
//! daemon, where `holding.rs` and `carrying.rs` stand the other window in with a socket and a
//! hand-written row, because until this a test process could hold only one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use muster::proto::{
    ClosePane, CreateTab, Event, FocusAsking, FocusHistory, FocusPane, MoveTab, OpenWindow,
    Quitting, ReadWindow, Request, Response, SplitPane, Startup, ToggleSidebar, WindowFocus, event,
    request, response,
};
use muster_core::composition::holding::from_toml;
use muster_daemon_proto::AgentState;
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn what_a_window_is_sent_names_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    start(&daemon, "window-7");
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));

    until(
        "the window to be told what it shows",
        || showing_in("window-7").is_some(),
        || format!("views arrived for {:?}", shown_windows()),
    );
    assert!(
        shown_windows().iter().all(|window| window == "window-7"),
        "a view was sent without the name of the window it is for, so a shell holding two could \
         not tell which one to draw it in: {:?}",
        shown_windows()
    );
    let rosters = windows_sent(|payload| matches!(payload, event::Payload::RosterChanged(_)));
    assert!(
        !rosters.is_empty() && rosters.iter().all(|window| window == "window-7"),
        "a roster was sent without the name of the window it is for: {rosters:?}"
    );
}

#[test]
fn a_request_for_a_window_this_process_has_not_got_is_refused() {
    let _turn = muster::testing::fresh_session();
    let response = answer(
        &Request::new(request::Payload::ToggleSidebar(ToggleSidebar {})).for_window("window-99"),
    );
    match response.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("window-99"),
            "the refusal does not say which window it could not find: {}",
            failure.reason
        ),
        other => panic!(
            "a request for a window nobody opened was carried out somewhere instead: {other:?}"
        ),
    }
}

/// A second window opens beside the first and starts with a tab of its own, and neither lists the
/// other's.
#[test]
fn two_windows_in_one_process_each_list_their_own_tabs() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);

    assert_eq!(listed("window-1"), vec![first.clone()], "the first window lists another's tab");
    assert_eq!(listed("window-2"), vec![second.clone()], "the second window lists another's tab");
    let record = holders(&daemon);
    assert_eq!(
        record.get(&first).map(String::as_str),
        Some("window-1"),
        "the record does not give the first window its tab: {record:?}"
    );
    assert_eq!(
        record.get(&second).map(String::as_str),
        Some("window-2"),
        "the record does not give the second window its tab: {record:?}"
    );

    // Each remembers itself in its own file, which is what a relaunch reopens each from.
    for (window, tab) in [("window-1", &first), ("window-2", &second)] {
        let path = arrangement(&daemon, window);
        let written = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{window} wrote nothing to {}: {e}", path.display()));
        assert!(
            written.contains(tab.as_str()),
            "{window}'s arrangement does not name {tab}:\n{written}"
        );
    }
}

/// A tab made from one window is that window's, whichever window is in front.
#[test]
fn a_tab_made_from_a_window_joins_that_window() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);
    focus_window("window-2");

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::CreateTab(CreateTab { take_focus: true, ..CreateTab::default() }),
    )));

    let made: Vec<String> = listed("window-1").into_iter().filter(|tab| tab != &first).collect();
    assert_eq!(made.len(), 1, "the first window lists {:?}", listed("window-1"));
    assert_eq!(
        listed("window-2"),
        vec![second],
        "the tab the first window made went to the window in front instead"
    );
    assert_eq!(
        showing_in("window-1"),
        made.first().cloned(),
        "the first window did not move onto its tab"
    );
}

/// A tab nobody asked for joins whichever of the two windows came to the front last.
#[test]
fn a_tab_nobody_asked_for_joins_the_window_in_front() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    focus_window("window-2");
    make(&mut daemon.connect(), create("p-outside-1", in_new_tab("t-outside-1")));
    until(
        "the window in front to take the tab nobody asked for",
        || listed("window-2").contains(&"t-outside-1".to_string()),
        || format!("window-2 lists {:?}, window-1 {:?}", listed("window-2"), listed("window-1")),
    );
    assert!(!listed("window-1").contains(&"t-outside-1".to_string()), "both windows took the tab");

    focus_window("window-1");
    make(&mut daemon.connect(), create("p-outside-2", in_new_tab("t-outside-2")));
    until(
        "the window now in front to take the next one",
        || listed("window-1").contains(&"t-outside-2".to_string()),
        || format!("window-1 lists {:?}, window-2 {:?}", listed("window-1"), listed("window-2")),
    );
    assert!(!listed("window-2").contains(&"t-outside-2".to_string()), "both windows took the tab");
}

/// Moving a tab from one window to another in the same process moves it at once, with nothing
/// waiting on the record's file to be read back.
#[test]
fn a_tab_moves_between_two_windows_here() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: first.clone(),
            window: "window-2".to_string(),
        }),
    )));

    assert!(listed("window-1").is_empty(), "the first window still lists {:?}", listed("window-1"));
    let mut theirs = listed("window-2");
    theirs.sort();
    let mut expected = vec![first.clone(), second];
    expected.sort();
    assert_eq!(theirs, expected, "the second window did not take the tab");
    assert_eq!(
        holders(&daemon).get(&first).map(String::as_str),
        Some("window-2"),
        "the record still gives the moved tab to the window it left"
    );
}

/// Asked what it holds, a window describes the other one in the same process with its tabs, the
/// way it describes a window in another process.
#[test]
fn a_window_describes_the_other_one_beside_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (_, second) = two_windows(&daemon);

    let window = read_window("window-1");
    let other = window.windows.iter().find(|other| other.name == "window-2").unwrap_or_else(|| {
        panic!("the first window does not mention the second: {:?}", window.windows)
    });
    assert!(
        other.tabs.iter().any(|tab| tab.tab_id == second),
        "the second window is described without its tab: {other:?}"
    );
    assert_eq!(other.pid, std::process::id(), "an open window here is described as closed");
}

/// Seen means on screen in the window in front. An agent finishing in the window behind is `done`
/// until that window comes forward, and then it has been seen.
#[test]
fn an_agent_in_the_window_behind_is_seen_when_that_window_comes_forward() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let (first, _) = two_windows(&daemon);
    assert_eq!(first, "t1", "the first window did not open onto the agent's tab");
    focus_window("window-2");

    daemon.set_agent_state("p1", AgentState::Working);
    until_state("p1", "working");
    daemon.set_agent_state("p1", AgentState::Idle);
    until_state("p1", "done");

    focus_window("window-1");
    until_state("p1", "idle");
}

/// Going to a pane in the other window's tab, from the window in front, is answered by the window
/// holding it: its keyboard moves there and it comes forward, and the window asked keeps its own.
#[test]
fn a_pane_in_the_other_window_is_gone_to_there() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let theirs = keyboard_in("window-2").expect("the second window opened onto a pane");
    let split = split_in("window-2");
    let ours = keyboard_in("window-1");

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::FocusPane(FocusPane { pane_id: theirs.clone(), ..FocusPane::default() }),
    )));

    until(
        "the second window's keyboard to move back onto its first pane",
        || keyboard_in("window-2").as_deref() == Some(theirs.as_str()),
        || {
            format!(
                "window-2's keyboard is on {:?}, not {theirs} or {split}",
                keyboard_in("window-2")
            )
        },
    );
    assert_eq!(keyboard_in("window-1"), ours, "the window asked moved its own keyboard too");
    assert_eq!(
        windows_sent(|payload| matches!(payload, event::Payload::RaiseWindow(_))),
        vec!["window-2".to_string()],
        "the window holding the pane was not brought forward, or another one was"
    );
}

/// A request naming no window, sent from a pane, is about the window holding that pane's tab and
/// not whichever is in front.
#[test]
fn a_request_from_a_pane_is_about_that_panes_window() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let theirs = keyboard_in("window-2").expect("the second window opened onto a pane");
    focus_window("window-1");

    let mut asked = Request::new(request::Payload::ReadWindow(ReadWindow {}));
    asked.from_pane = theirs;
    match answer(&asked).payload {
        Some(response::Payload::Window(window)) => assert_eq!(
            window.name, "window-2",
            "a request from a pane in the second window was answered by the one in front"
        ),
        other => panic!("reading the window answered {other:?}"),
    }
}

/// A window name nobody here has is refused even when the request names a pane that decides which
/// window answers: the caller meant some window, and was wrong about which.
#[test]
fn a_window_nobody_has_is_refused_even_naming_a_pane() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let theirs = keyboard_in("window-2").expect("the second window opened onto a pane");

    let response = answer(&in_window(
        "window-99",
        request::Payload::FocusPane(FocusPane { pane_id: theirs, ..FocusPane::default() }),
    ));
    match response.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("window-99"),
            "the refusal does not name the window it could not find: {}",
            failure.reason
        ),
        other => panic!("a request for a window nobody opened was carried out: {other:?}"),
    }
}

/// A change to a pane in the other window's tab is made there, and the window it was sent to keeps
/// showing its own tab.
#[test]
fn a_change_to_the_other_windows_pane_is_made_there() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);
    let theirs = keyboard_in("window-2").expect("the second window opened onto a pane");
    split_in("window-2");

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::ClosePane(ClosePane { pane_id: theirs.clone(), ..ClosePane::default() }),
    )));

    until(
        "the pane to close",
        || !panes_in("window-2").contains(&theirs),
        || format!("window-2 still holds {:?}", panes_in("window-2")),
    );
    assert_eq!(listed("window-1"), vec![first], "the window asked took the other one's tab");
    assert_eq!(listed("window-2"), vec![second], "the window holding the pane lost its tab");
}

/// ⌘⇧A in one window goes to an agent waiting in the other window's tab, in that window, which
/// comes forward.
#[test]
fn going_to_an_agent_asking_goes_to_the_window_holding_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let (first, _) = two_windows(&daemon);
    assert_eq!(first, "t1", "the first window did not open onto the agent's tab");
    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab { tab_id: first, window: "window-2".to_string() }),
    )));
    focus_window("window-1");
    daemon.set_agent_state("p1", AgentState::Blocked);
    until_state("p1", "blocked");

    match answer(&in_window("window-1", request::Payload::FocusAsking(FocusAsking {}))).payload {
        Some(response::Payload::Asking(went)) => {
            assert_eq!(went.pane_id, "p1", "went somewhere other than the agent asking");
        }
        other => panic!("going to the agent asking answered {other:?}"),
    }
    until(
        "the second window's keyboard to move onto the agent",
        || keyboard_in("window-2").as_deref() == Some("p1"),
        || format!("window-2's keyboard is on {:?}", keyboard_in("window-2")),
    );
    assert_eq!(
        windows_sent(|payload| matches!(payload, event::Payload::RaiseWindow(_))),
        vec!["window-2".to_string()],
        "the window holding the agent was not brought forward, or another one was"
    );
}

/// Back in one window goes to the pane that window's keyboard was on before, whatever was done in
/// the other window in between.
#[test]
fn each_window_walks_back_through_its_own_panes() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let ours = keyboard_in("window-1").expect("the first window opened onto a pane");
    split_in("window-1");
    focus_window("window-2");
    let theirs = split_in("window-2");
    focus_window("window-1");

    match answer(&in_window(
        "window-1",
        request::Payload::FocusHistory(FocusHistory { forward: false }),
    ))
    .payload
    {
        Some(response::Payload::Went(went)) => assert_eq!(
            went.pane_id, ours,
            "back in the first window went somewhere other than the pane it was on before"
        ),
        other => panic!("going back answered {other:?}"),
    }
    until(
        "the first window's keyboard to go back",
        || keyboard_in("window-1").as_deref() == Some(ours.as_str()),
        || format!("window-1's keyboard is on {:?}", keyboard_in("window-1")),
    );
    assert_eq!(
        keyboard_in("window-2").as_deref(),
        Some(theirs.as_str()),
        "going back in the first window moved the second window's keyboard"
    );
}

/// Quitting the process closes every window in it, each keeping its tabs for when it reopens.
#[test]
fn quitting_closes_every_window_here() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);

    assert_ok(&answer(&Request::new(request::Payload::Quitting(Quitting::default()))));

    let record = from_toml(&std::fs::read_to_string(record(&daemon)).unwrap_or_default())
        .expect("the record reads back");
    for name in ["window-1", "window-2"] {
        let row = record
            .windows()
            .find(|window| window.name.as_str() == name)
            .unwrap_or_else(|| panic!("the record lost {name} on the way out"));
        assert_eq!(row.pid, 0, "{name} is still recorded as open after quitting: {row:?}");
    }
    let kept = holders(&daemon);
    assert_eq!(
        kept.get(&first).map(String::as_str),
        Some("window-1"),
        "window-1 lost its tab: {kept:?}"
    );
    assert_eq!(
        kept.get(&second).map(String::as_str),
        Some("window-2"),
        "window-2 lost its tab: {kept:?}"
    );
}

/// The first window open onto the daemon's one tab, and a second opened beside it onto a tab it
/// asked for. Answers the two tabs, first window's first.
fn two_windows(daemon: &Daemon) -> (String, String) {
    start(daemon, "window-1");
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));
    until(
        "the first window to open onto a tab",
        || showing_in("window-1").is_some(),
        || format!("views arrived for {:?}", shown_windows()),
    );
    let first = showing_in("window-1").expect("just waited for it");

    let opened = answer(&Request::new(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement(daemon, "window-2").to_string_lossy().into_owned(),
        ..OpenWindow::default()
    })));
    match opened.payload {
        Some(response::Payload::Opened(opened)) => assert_eq!(opened.window, "window-2"),
        other => panic!("opening a second window answered {other:?}"),
    }
    // A window holding nothing asks this machine for a tab of its own, and the daemon's answer
    // arrives on its own thread.
    until(
        "the second window to open onto a tab of its own",
        || showing_in("window-2").is_some_and(|tab| tab != first),
        || {
            format!(
                "window-2 shows {:?} and lists {:?}",
                showing_in("window-2"),
                listed("window-2")
            )
        },
    );
    let second = showing_in("window-2").expect("just waited for it");
    (first, second)
}

fn start(daemon: &Daemon, window: &str) {
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: arrangement(daemon, window).to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
}

fn focus_window(window: &str) {
    assert_ok(&answer(&in_window(
        window,
        request::Payload::WindowFocus(WindowFocus { focused: true }),
    )));
}

fn arrangement(daemon: &Daemon, window: &str) -> PathBuf {
    daemon.root().join(format!("windows/{window}.toml"))
}

fn record(daemon: &Daemon) -> PathBuf {
    daemon.root().join("holding/tabs.toml")
}

/// Every tab the record names, with the window holding it.
fn holders(daemon: &Daemon) -> BTreeMap<String, String> {
    holders_in(&record(daemon))
}

fn holders_in(path: &Path) -> BTreeMap<String, String> {
    let holders = from_toml(&std::fs::read_to_string(path).unwrap_or_default())
        .expect("the record the windows write reads back");
    holders
        .windows()
        .flat_map(|window| {
            holders.held_by(&window.name).map(|tab| (tab.to_string(), window.name.to_string()))
        })
        .collect()
}

fn read_window(window: &str) -> muster::proto::Window {
    match answer(&in_window(window, request::Payload::ReadWindow(ReadWindow {}))).payload {
        Some(response::Payload::Window(answer)) => answer,
        other => panic!("asking {window} what it shows answered {other:?}"),
    }
}

/// The tabs a window lists, in its order.
fn listed(window: &str) -> Vec<String> {
    read_window(window)
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .map(|tab| tab.tab_id.clone())
        .collect()
}

/// Waits for a pane's agent to read `state`, as the first window paints it.
fn until_state(pane: &str, state: &str) {
    let painted = || {
        read_window("window-1")
            .panes
            .iter()
            .find(|agent| agent.pane_id == pane)
            .map(|agent| agent.state.clone())
    };
    until(
        &format!("{pane} to read {state}"),
        || painted().as_deref() == Some(state),
        || format!("{pane} reads {:?}", painted()),
    );
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(event);
}

fn forget_events() {
    EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// The tab a window was last told it shows, from the views sent to it.
fn showing_in(window: &str) -> Option<String> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let view = events.iter().rev().find_map(|event| match &event.payload {
        Some(event::Payload::ViewChanged(view)) if event.window == window => Some(view),
        _ => None,
    })?;
    let region = view.regions.first()?;
    region.root.as_ref()?;
    Some(region.tab_id.clone())
}

/// Every window a view has been sent to.
fn shown_windows() -> Vec<String> {
    windows_sent(|payload| matches!(payload, event::Payload::ViewChanged(_)))
}

/// Every window an event of one kind has been sent to, in the order they were sent.
fn windows_sent(kind: impl Fn(&event::Payload) -> bool) -> Vec<String> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut windows: Vec<String> = events
        .iter()
        .filter(|event| event.payload.as_ref().is_some_and(&kind))
        .map(|event| event.window.clone())
        .collect();
    windows.dedup();
    windows
}

/// The pane a window's keyboard is on, from the last view sent to it.
fn keyboard_in(window: &str) -> Option<String> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let view = events.iter().rev().find_map(|event| match &event.payload {
        Some(event::Payload::ViewChanged(view)) if event.window == window => Some(view),
        _ => None,
    })?;
    let region = view.regions.iter().find(|region| region.region_id == view.focused_region)?;
    Some(region.pane_id.clone()).filter(|pane| !pane.is_empty())
}

/// Every pane a window lists.
fn panes_in(window: &str) -> Vec<String> {
    read_window(window)
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter().map(|pane| pane.pane_id.clone()))
        .collect()
}

/// Splits the pane a window's keyboard is on, moves the keyboard into the new one, and names it.
fn split_in(window: &str) -> String {
    let response = answer(&in_window(
        window,
        request::Payload::SplitPane(SplitPane {
            side: "right".to_string(),
            take_focus: true,
            ..SplitPane::default()
        }),
    ));
    let made = match response.payload {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("a split in {window} answered {other:?}"),
    };
    until(
        &format!("{window}'s keyboard to move into the split"),
        || keyboard_in(window).as_deref() == Some(made.as_str()),
        || format!("{window}'s keyboard is on {:?}", keyboard_in(window)),
    );
    made
}

fn in_window(window: &str, payload: request::Payload) -> Request {
    Request::new(payload).for_window(window)
}

fn answer(request: &Request) -> Response {
    let reply = muster::dispatch(&request.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
