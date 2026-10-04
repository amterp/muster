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
    AskForWindow, AskToCloseWindow, ClosePane, CloseWindow, CreateTab, Event, FocusAsking,
    FocusHistory, FocusPane, FocusTab, MoveTab, OpenWindow, Quitting, ReadAsking, ReadReopening,
    ReadWindow, ReattachPane, Request, Response, SplitPane, Startup, StillOpen, ToggleSidebar,
    ViewNode, WindowFocus, event, request, response, view_node,
};
use muster_core::composition::holding::from_toml;
use muster_daemon_proto::{self as daemon_proto, AgentState, session_request};
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

/// A tab moved to a window by name joins the end of that window's list without coming on screen,
/// whichever window was asked, and its panes keep running.
///
/// From outside a pane the CLI's request reaches whichever window is in front. Only a move naming
/// no window - "bring it here" - is somebody looking at the window it lands in.
#[test]
fn a_tab_moved_to_a_window_by_name_joins_its_list_without_coming_on_screen() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);
    let panes = panes_in("window-1").len() + panes_in("window-2").len();

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: second.clone(),
            window: "window-1".to_string(),
        }),
    )));

    assert_eq!(listed("window-1"), vec![first.clone(), second], "the tab is not at the end");
    assert_eq!(showing_in("window-1"), Some(first), "a tab moved here by name came on screen");
    assert_eq!(
        panes_in("window-1").len() + panes_in("window-2").len(),
        panes,
        "moving a tab between windows ended a pane"
    );
}

/// A tab moved to this app's pid goes to the window in front: what a pid meant when each window
/// was a process of its own, and the only window a pid can still name.
#[test]
fn a_tab_moved_to_the_apps_pid_goes_to_the_window_in_front() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, _) = two_windows(&daemon);
    focus_window("window-2");

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: first.clone(),
            window: std::process::id().to_string(),
        }),
    )));

    assert!(listed("window-2").contains(&first), "the tab did not go to the window in front");
    assert!(!listed("window-1").contains(&first), "the tab stayed where it was");
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

/// Asked for its layout, a window lays out the other open window's tabs too, so every pane it
/// lists has a frame in its own tab, whichever window holds that tab.
#[test]
fn a_windows_layout_covers_the_tabs_of_the_window_beside_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);

    let window = match answer(&in_window(
        "window-1",
        request::Payload::ReadWindow(ReadWindow { layout: true }),
    ))
    .payload
    {
        Some(response::Payload::Window(window)) => window,
        other => panic!("reading the layout answered {other:?}"),
    };
    let laid_out: Vec<&str> = window.layouts.iter().map(|layout| layout.tab_id.as_str()).collect();
    assert_eq!(laid_out.first(), Some(&first.as_str()), "this window's own tab comes first");
    let other = window
        .layouts
        .iter()
        .find(|layout| layout.tab_id == second)
        .unwrap_or_else(|| panic!("the other window's tab is not laid out: {laid_out:?}"));
    assert!(
        !other.places.is_empty(),
        "the other window's tab is laid out with no panes: {other:?}"
    );
}

/// A closed window's tabs are laid out too, from its record, so its panes have frames in their
/// tabs as an open window's do.
#[test]
fn a_windows_layout_covers_the_tabs_of_a_closed_window() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (_, second) = two_windows(&daemon);
    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));

    let window = match answer(&in_window(
        "window-1",
        request::Payload::ReadWindow(ReadWindow { layout: true }),
    ))
    .payload
    {
        Some(response::Payload::Window(window)) => window,
        other => panic!("reading the layout answered {other:?}"),
    };
    let laid_out: Vec<&str> = window.layouts.iter().map(|layout| layout.tab_id.as_str()).collect();
    let closed = window
        .layouts
        .iter()
        .find(|layout| layout.tab_id == second)
        .unwrap_or_else(|| panic!("the closed window's tab is not laid out: {laid_out:?}"));
    let [place] = closed.places.as_slice() else {
        panic!("the closed window's one pane is not placed once: {closed:?}");
    };
    assert_eq!(
        (place.x, place.y, place.width, place.height),
        (0.0, 0.0, 1.0, 1.0),
        "a tab holding one pane is that pane"
    );
}

/// A pane's bridge restarts are counted per window: a tab moved here from the window beside this
/// one brings none of that window's replacements with it, so its first bridge here takes nothing
/// over, and a replacement asked for here counts from there.
#[test]
fn a_moved_tabs_bridges_are_counted_from_zero_in_the_window_it_moved_to() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, _) = two_windows(&daemon);
    let pane = keyboard_in("window-1").expect("the first window opens with the keyboard on a pane");

    assert_ok(&answer(&in_window("window-1", reattach(&pane))));
    until(
        "the first window to be given a second bridge for its pane",
        || restarts_in("window-1", &pane) == Some(1),
        || format!("window-1 counts {:?}", restarts_in("window-1", &pane)),
    );

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: first.clone(),
            window: "window-2".to_string(),
        }),
    )));
    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::FocusTab(FocusTab { tab_id: first.clone(), ..FocusTab::default() }),
    )));
    until(
        "the second window to show the moved pane with no replacements of its own",
        || restarts_in("window-2", &pane) == Some(0),
        || format!("window-2 counts {:?}", restarts_in("window-2", &pane)),
    );

    assert_ok(&answer(&in_window("window-2", reattach(&pane))));
    until(
        "a replacement in the second window to count from there",
        || restarts_in("window-2", &pane) == Some(1),
        || format!("window-2 counts {:?}", restarts_in("window-2", &pane)),
    );
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

    let mut asked = Request::new(request::Payload::ReadWindow(ReadWindow::default()));
    asked.from_pane = theirs;
    match answer(&asked).payload {
        Some(response::Payload::Window(window)) => assert_eq!(
            window.name, "window-2",
            "a request from a pane in the second window was answered by the one in front"
        ),
        other => panic!("reading the window answered {other:?}"),
    }
}

/// A pane's environment names the window it was made in, and its tab may since have moved. A
/// request from it that names nothing - a split of "this pane" - is about the window holding its
/// tab now, which nothing has to carry it to.
#[test]
fn a_request_from_a_pane_whose_tab_moved_is_about_the_window_it_moved_to() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, _) = two_windows(&daemon);
    let pane = keyboard_in("window-1").expect("the first window opened onto a pane");
    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: first.clone(),
            window: "window-2".to_string(),
        }),
    )));
    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::FocusTab(FocusTab { tab_id: first, ..FocusTab::default() }),
    )));
    focus_window("window-1");

    let mut split = Request::new(request::Payload::SplitPane(SplitPane {
        side: "right".to_string(),
        ..SplitPane::default()
    }));
    split.from_pane = pane;
    let made = match answer(&split).payload {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("splitting from the moved pane answered {other:?}"),
    };
    assert!(
        panes_in("window-2").contains(&made),
        "the split was not made in the window the pane's tab moved to: {:?}",
        panes_in("window-2")
    );
    assert!(!panes_in("window-1").contains(&made), "the split was made in the window in front");
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

/// Asked what going to the agent asking would do, a window names the agent in the window beside
/// it, the same one going there goes to, and moves nothing: the menu item that greys itself out
/// on this answer is in every window.
#[test]
fn what_going_to_an_agent_asking_would_do_names_the_other_windows_agent() {
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
    let keyboard = keyboard_in("window-2");

    match answer(&in_window("window-1", request::Payload::ReadAsking(ReadAsking {}))).payload {
        Some(response::Payload::Asking(asking)) => {
            assert_eq!(asking.pane_id, "p1", "named somewhere other than the agent asking");
        }
        other => panic!("asking what going to the agent would do answered {other:?}"),
    }
    assert_eq!(keyboard_in("window-2"), keyboard, "asking moved the other window's keyboard");
    assert!(
        windows_sent(|payload| matches!(payload, event::Payload::RaiseWindow(_))).is_empty(),
        "asking brought a window forward"
    );
}

/// A problem's Reattach is listed in every window, so it can be clicked in a window that does
/// not hold the pane. The bridge is replaced where the pane is drawn, and the window clicked in
/// is left as it was.
#[test]
fn reattaching_the_other_windows_pane_reattaches_it_there() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, _) = two_windows(&daemon);
    let pane = keyboard_in("window-1").expect("the first window opens with the keyboard on a pane");
    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab {
            tab_id: first.clone(),
            window: "window-2".to_string(),
        }),
    )));
    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::FocusTab(FocusTab { tab_id: first.clone(), ..FocusTab::default() }),
    )));
    until(
        "the second window to draw the moved pane",
        || restarts_in("window-2", &pane) == Some(0),
        || format!("window-2 counts {:?}", restarts_in("window-2", &pane)),
    );
    forget_events();

    assert_ok(&answer(&in_window("window-1", reattach(&pane))));
    until(
        "the second window to be given a new bridge for its pane",
        || restarts_in("window-2", &pane) == Some(1),
        || format!("window-2 counts {:?}", restarts_in("window-2", &pane)),
    );
    assert_eq!(showing_in("window-2"), Some(first), "the second window changed tab");
    assert!(
        !windows_sent(|payload| matches!(payload, event::Payload::ViewChanged(_)))
            .contains(&"window-1".to_string()),
        "the window clicked in changed what it shows"
    );
    assert!(
        windows_sent(|payload| matches!(payload, event::Payload::RaiseWindow(_))).is_empty(),
        "reattaching brought a window forward"
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

/// Back steps over a pane whose tab has moved to the other window, rather than going to it there.
#[test]
fn back_steps_over_a_pane_moved_to_the_other_window() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let ours = keyboard_in("window-1").expect("the first window opened onto a pane");
    let moved = new_tab_in("window-1");
    new_tab_in("window-1");
    let theirs = keyboard_in("window-2");
    let tab = read_window("window-1")
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .find(|tab| tab.panes.iter().any(|pane| pane.pane_id == moved))
        .map(|tab| tab.tab_id.clone())
        .expect("the pane is in one of the first window's tabs");
    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::MoveTab(MoveTab { tab_id: tab, window: "window-2".to_string() }),
    )));

    match answer(&in_window(
        "window-1",
        request::Payload::FocusHistory(FocusHistory { forward: false }),
    ))
    .payload
    {
        Some(response::Payload::Went(went)) => assert_eq!(
            went.pane_id, ours,
            "back did not step over the pane whose tab went to the other window"
        ),
        other => panic!("going back answered {other:?}"),
    }
    assert_eq!(keyboard_in("window-2"), theirs, "going back moved the other window's keyboard");
}

/// Quitting is not closing: every window here stays open in the record, keeping its tabs, and the
/// next launch is told to open them all.
#[test]
fn quitting_leaves_every_window_here_for_the_next_launch() {
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
        assert_ne!(row.pid, 0, "quitting closed {name}, so the next launch would not open it");
    }
    let kept = holders(&daemon);
    assert_eq!(kept.get(&first).map(String::as_str), Some("window-1"), "{kept:?}");
    assert_eq!(kept.get(&second).map(String::as_str), Some("window-2"), "{kept:?}");
    // Both, in whatever order: two focuses a moment apart can share a millisecond here, and which
    // order a launch opens them in is the record's to say (`corpus/conformance/tab-holding.json`).
    let mut reopened = reopening(&daemon);
    reopened.sort();
    assert_eq!(
        reopened,
        vec![arrangement_text(&daemon, "window-1"), arrangement_text(&daemon, "window-2")],
        "the next launch is not told to open both windows"
    );
}

/// Ending the sessions on the way out ends every tab, so it closes the windows too: bringing them
/// back would be windows onto tabs that are gone.
#[test]
fn quitting_and_ending_the_sessions_closes_every_window() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    assert_ok(&answer(&Request::new(request::Payload::Quitting(Quitting {
        close_sessions: true,
    }))));

    assert!(reopening(&daemon).is_empty(), "the next launch would reopen windows onto ended tabs");
}

/// Closing one window leaves the other open and the closed one holding its tabs, which the open
/// one does not take.
#[test]
fn closing_one_window_leaves_the_other_and_keeps_its_tabs() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);

    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));

    let kept = holders(&daemon);
    assert_eq!(kept.get(&second).map(String::as_str), Some("window-2"), "{kept:?}");
    assert_eq!(listed("window-1"), vec![first], "the open window took the closed one's tab");
    // Only the closed window is asked about: this test listens on no socket, so the window still
    // open here cannot be told from one that ended.
    assert!(
        !reopening(&daemon).contains(&arrangement_text(&daemon, "window-2")),
        "a window somebody closed would be reopened by the next launch"
    );
    match answer(&in_window("window-2", request::Payload::ToggleSidebar(ToggleSidebar {}))).payload
    {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("muster window reopen window-2"),
            "the refusal does not say how to open the window again: {}",
            failure.reason
        ),
        other => panic!("a request for a closed window was carried out: {other:?}"),
    }
}

/// A closed window is sent nothing: a tab nobody asked for does not join it, and nothing is drawn
/// for it.
#[test]
fn a_closed_window_is_sent_nothing() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));
    let sent = sent_to("window-2");

    make(&mut daemon.connect(), create("p-outside", in_new_tab("t-outside")));
    until(
        "the open window to take the tab nobody asked for",
        || listed("window-1").contains(&"t-outside".to_string()),
        || format!("window-1 lists {:?}", listed("window-1")),
    );

    assert_eq!(sent_to("window-2"), sent, "a closed window was sent a view or a roster");
}

/// The window in front closing leaves a request that names no window to the window still open.
#[test]
fn the_window_in_front_closing_leaves_the_other_in_front() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    focus_window("window-1");
    focus_window("window-2");

    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));

    match answer(&Request::new(request::Payload::ReadWindow(ReadWindow::default()))).payload {
        Some(response::Payload::Window(window)) => assert_eq!(window.name, "window-1"),
        other => panic!("reading the window answered {other:?}"),
    }
}

/// `muster window close` asks the shell to close the window it names, and only that one, the
/// way its close button would.
#[test]
fn a_window_asked_to_close_is_closed_by_the_shell() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::AskToCloseWindow(AskToCloseWindow {}),
    )));

    assert_eq!(
        windows_sent(|payload| matches!(payload, event::Payload::ShutWindow(_))),
        vec!["window-2".to_string()],
        "the shell was not asked to close exactly the window named"
    );
}

/// Closing the last window open is a quit, so asking for it from outside the shell is refused
/// rather than ending every window.
#[test]
fn the_last_window_open_is_not_closed_when_asked() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));

    match answer(&in_window("window-1", request::Payload::AskToCloseWindow(AskToCloseWindow {})))
        .payload
    {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("only window open") && failure.reason.contains("cmd+q"),
            "the refusal does not say why, or how to quit instead: {}",
            failure.reason
        ),
        other => panic!("closing the last window was asked for: {other:?}"),
    }
    // A window already closed has nothing to close, and that is not a failure.
    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::AskToCloseWindow(AskToCloseWindow {}),
    )));
    assert!(
        windows_sent(|payload| matches!(payload, event::Payload::ShutWindow(_))).is_empty(),
        "the shell was asked to close a window"
    );
}

/// Two closes asked at once of an app with two windows close one: the second counts the first as
/// done, rather than both seeing two windows open and together closing the last.
#[test]
fn two_closes_asked_at_once_leave_a_window_open() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::AskToCloseWindow(AskToCloseWindow {}),
    )));
    let second =
        answer(&in_window("window-1", request::Payload::AskToCloseWindow(AskToCloseWindow {})));

    assert!(
        matches!(second.payload, Some(response::Payload::Failure(_))),
        "the second close was asked for while the first was still under way: {second:?}"
    );
    assert_eq!(
        windows_sent(|payload| matches!(payload, event::Payload::ShutWindow(_))),
        vec!["window-2".to_string()]
    );
}

/// A close the shell could not carry out - a sheet up in the window - leaves the window open
/// rather than closing for good: it can be asked again, and the window beside it is not refused
/// as the last one open.
#[test]
fn a_close_the_shell_could_not_carry_out_leaves_the_window_open() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);
    let shut = || {
        let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        events
            .iter()
            .filter(|event| matches!(event.payload, Some(event::Payload::ShutWindow(_))))
            .map(|event| event.window.clone())
            .collect::<Vec<_>>()
    };

    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::AskToCloseWindow(AskToCloseWindow {}),
    )));
    assert_ok(&answer(&in_window("window-2", request::Payload::StillOpen(StillOpen {}))));
    assert_ok(&answer(&in_window(
        "window-2",
        request::Payload::AskToCloseWindow(AskToCloseWindow {}),
    )));
    assert_eq!(
        shut(),
        vec!["window-2".to_string(), "window-2".to_string()],
        "a window whose close did not happen was not asked to close again"
    );

    assert_ok(&answer(&in_window("window-2", request::Payload::StillOpen(StillOpen {}))));
    let beside =
        answer(&in_window("window-1", request::Payload::AskToCloseWindow(AskToCloseWindow {})));
    assert!(
        !matches!(beside.payload, Some(response::Payload::Failure(_))),
        "with window-2 still open, closing window-1 was refused as the last window: {beside:?}"
    );
    assert_eq!(shut().last().map(String::as_str), Some("window-1"));
}

/// Going to a closed window's tab asks for that window back, and opening its arrangement again
/// brings it back onto that tab.
#[test]
fn a_closed_window_comes_back_onto_its_tabs() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (_, second) = two_windows(&daemon);
    assert_ok(&answer(&in_window("window-2", request::Payload::CloseWindow(CloseWindow {}))));

    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::FocusTab(FocusTab { tab_id: second.clone(), ..FocusTab::default() }),
    )));
    assert_eq!(
        asked_for(),
        vec![("window-2".to_string(), second.clone(), false)],
        "going to the closed window's tab did not ask for that window back"
    );

    let opened = answer(&Request::new(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement_text(&daemon, "window-2"),
        show: second.clone(),
        ..OpenWindow::default()
    })));
    match opened.payload {
        Some(response::Payload::Opened(opened)) => assert_eq!(opened.window, "window-2"),
        other => panic!("opening the closed window again answered {other:?}"),
    }
    until(
        "the window to come back onto its tab",
        || showing_in("window-2").as_deref() == Some(second.as_str()),
        || {
            format!(
                "window-2 shows {:?} and lists {:?}",
                showing_in("window-2"),
                listed("window-2")
            )
        },
    );
}

/// Somebody outside the app asking for a window is passed to the shell, which makes windows - and
/// refused when it comes from another install, whose windows follow another daemon.
#[test]
fn asking_for_a_window_asks_the_shell() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    assert_ok(&answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: muster_daemon_proto::install::INSTALL.to_string(),
        fresh: true,
        ..AskForWindow::default()
    }))));
    assert_eq!(asked_for(), vec![(String::new(), String::new(), true)]);

    let refused = answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: "someone-else".to_string(),
        fresh: true,
        ..AskForWindow::default()
    })));
    assert!(
        matches!(refused.payload, Some(response::Payload::Failure(_))),
        "another install's request for a window was passed on: {refused:?}"
    );
    assert_eq!(asked_for().len(), 1, "another install's request reached the shell");
}

/// A launch handed to the running app with a pane nobody holds is told so, rather than told it
/// was handed over as though the pane were now on screen.
#[test]
fn a_launch_for_a_pane_nobody_holds_is_told_so() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    let answered = answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: muster_daemon_proto::install::INSTALL.to_string(),
        show: "p-nobody".to_string(),
        any: true,
        ..AskForWindow::default()
    })));

    match answered.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("p-nobody"),
            "the refusal does not name the pane: {}",
            failure.reason
        ),
        other => panic!("a launch for a pane nobody holds was answered {other:?}"),
    }
}

/// `muster window new --daemon far` opens a window whose first tab is on that machine, where a
/// window asked for otherwise takes one on the first machine here.
#[test]
fn a_window_asked_for_on_a_machine_opens_its_first_tab_there() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let near = Daemon::start_built();
    let far = Daemon::start_built();
    let config = near.root().join("two-machines.toml");
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"local\"\nsocket = {:?}\n\n[[daemon]]\nid = \"far\"\nsocket = {:?}\n",
            near.socket_path().to_string_lossy(),
            far.socket_path().to_string_lossy()
        ),
    )
    .expect("the harness root is writable");
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement(&near, "window-1").to_string_lossy().into_owned(),
        tab_holders_path: record(&near).to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));
    until(
        "the first window to open onto a tab",
        || showing_in("window-1").is_some(),
        || format!("views arrived for {:?}", shown_windows()),
    );

    // The other machine from the one the first window's tab is on, which is the one a window
    // asked for without naming a machine would take.
    let first = machine_showing_in("window-1").expect("just waited for it");
    let other = if first == "far" { "local" } else { "far" };

    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement(&near, "window-2").to_string_lossy().into_owned(),
        daemon: other.to_string(),
        ..OpenWindow::default()
    }))));

    until(
        "the second window to open onto a tab on the machine asked for",
        || machine_showing_in("window-2").as_deref() == Some(other),
        || format!("window-2 shows a tab on {:?}", machine_showing_in("window-2")),
    );
}

/// A window with nothing to show starts on the first machine the config names, waited for while
/// it attaches, rather than on whichever answered first.
#[test]
fn a_first_window_starts_on_the_machine_the_config_names_first() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let (first, second) = (Daemon::start_built(), Daemon::start_built());
    let slow = first.delaying_answers_where(subscribes, std::time::Duration::from_millis(800));
    let config = first.root().join("two-machines.toml");
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"second-named\"\nsocket = {:?}\n\n[[daemon]]\nid = \"a-first\"\n\
             socket = {:?}\n",
            slow.socket_path().to_string_lossy(),
            second.socket_path().to_string_lossy()
        ),
    )
    .expect("the harness root is writable");
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement(&first, "window-1").to_string_lossy().into_owned(),
        tab_holders_path: record(&first).to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));

    until(
        "the window to open onto a tab",
        || machine_showing_in("window-1").is_some(),
        || format!("views arrived for {:?}", shown_windows()),
    );
    assert_eq!(
        machine_showing_in("window-1").as_deref(),
        Some("second-named"),
        "the window started on the machine that answered first, not the one the config names first"
    );
}

/// A window asked for on a machine still attaching is opened, and its first tab is asked of that
/// machine once it answers, rather than the machine being refused as unknown.
#[test]
fn a_window_on_a_machine_still_attaching_waits_for_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let (near, far) = (Daemon::start_built(), Daemon::start_built());
    let slow = far.delaying_answers_where(subscribes, std::time::Duration::from_secs(3));
    let config = near.root().join("attaching.toml");
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"local\"\nsocket = {:?}\n\n[[daemon]]\nid = \"devenv\"\n\
             socket = {:?}\n",
            near.socket_path().to_string_lossy(),
            slow.socket_path().to_string_lossy()
        ),
    )
    .expect("the harness root is writable");
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement(&near, "window-1").to_string_lossy().into_owned(),
        tab_holders_path: record(&near).to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));

    assert_ok(&answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: muster_daemon_proto::install::INSTALL.to_string(),
        fresh: true,
        daemon: "devenv".to_string(),
        ..AskForWindow::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement(&near, "window-2").to_string_lossy().into_owned(),
        daemon: "devenv".to_string(),
        ..OpenWindow::default()
    }))));

    until(
        "the window to open onto a tab on the machine once it answers",
        || machine_showing_in("window-2").as_deref() == Some("devenv"),
        || format!("window-2 shows a tab on {:?}", machine_showing_in("window-2")),
    );
}

/// A launch handed over naming a pane before any window has opened keeps the pane for the window
/// the shell opens, since there is no window yet to go to it in.
#[test]
fn a_pane_asked_for_before_any_window_opens_is_kept_for_the_one_that_does() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    start(&daemon, "window-1");

    assert_ok(&answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: muster_daemon_proto::install::INSTALL.to_string(),
        show: "p-later".to_string(),
        any: true,
        ..AskForWindow::default()
    }))));

    assert_eq!(asked_for(), vec![(String::new(), "p-later".to_string(), false)]);
}

/// A first-named machine that will not attach is waited for only until its first attempt fails;
/// the window then starts on the next machine the config names, while the first is retried.
#[test]
fn a_first_window_passes_over_a_machine_that_failed_to_attach() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let working = Daemon::start_built();
    let nothing = working.root().join("nothing.sock");
    let config = working.root().join("first-missing.toml");
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"missing\"\nsocket = {:?}\n\n[[daemon]]\nid = \"working\"\n\
             socket = {:?}\n",
            nothing.to_string_lossy(),
            working.socket_path().to_string_lossy()
        ),
    )
    .expect("the harness root is writable");
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement(&working, "window-1").to_string_lossy().into_owned(),
        tab_holders_path: record(&working).to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));

    until(
        "the window to start on the machine that attached",
        || machine_showing_in("window-1").as_deref() == Some("working"),
        || format!("window-1 shows a tab on {:?}", machine_showing_in("window-1")),
    );
}

/// The window's subscribe, whose answer carries the daemon's state.
fn subscribes(request: &daemon_proto::Request) -> bool {
    matches!(
        &request.service,
        Some(daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(session_request::Request::Subscribe(_)),
        }))
    )
}

/// `muster window new --tab` opens a window onto a tab another window holds, which moves into it,
/// and asks no machine for a tab of its own.
#[test]
fn a_window_asked_for_onto_a_tab_takes_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, second) = two_windows(&daemon);
    assert_ok(&answer(&in_window(
        "window-1",
        request::Payload::CreateTab(CreateTab { take_focus: true, ..CreateTab::default() }),
    )));
    until(
        "the first window to hold a second tab",
        || listed("window-1").len() == 2,
        || format!("window-1 lists {:?}", listed("window-1")),
    );

    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow {
        state_path: arrangement(&daemon, "window-3").to_string_lossy().into_owned(),
        tab: first.clone(),
        ..OpenWindow::default()
    }))));

    until(
        "the third window to open onto the tab it asked for",
        || showing_in("window-3").as_deref() == Some(first.as_str()),
        || format!("window-3 shows {:?}", showing_in("window-3")),
    );
    assert_eq!(listed("window-3"), vec![first.clone()], "it made a tab of its own as well");
    assert!(!listed("window-1").contains(&first), "the tab stayed in the window it left");
    assert_eq!(listed("window-1").len(), 1, "the window it left lost more than that tab");
    assert_eq!(listed("window-2"), vec![second]);
}

/// A window's only tab is not taken for a new window: the window it left would show nothing, and
/// the tab is in a window of its own already.
#[test]
fn a_window_onto_another_windows_only_tab_is_refused() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let (first, _) = two_windows(&daemon);

    let answered = answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
        install: muster_daemon_proto::install::INSTALL.to_string(),
        fresh: true,
        tab: first.clone(),
        ..AskForWindow::default()
    })));

    match answered.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("only tab") && failure.reason.contains("window-1"),
            "the refusal does not say why: {}",
            failure.reason
        ),
        other => panic!("a window was asked for onto another window's only tab: {other:?}"),
    }
    assert!(asked_for().is_empty(), "the shell was asked for a window");
    assert_eq!(listed("window-1"), vec![first]);
}

/// A machine or a tab the app has not got is refused before the shell is asked for a window.
#[test]
fn a_window_onto_a_machine_or_tab_nobody_has_is_refused() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    two_windows(&daemon);

    for (daemon, tab, named) in [("nowhere", "", "nowhere"), ("", "t-nowhere", "t-nowhere")] {
        let answered = answer(&Request::new(request::Payload::AskForWindow(AskForWindow {
            install: muster_daemon_proto::install::INSTALL.to_string(),
            fresh: true,
            daemon: daemon.to_string(),
            tab: tab.to_string(),
            ..AskForWindow::default()
        })));
        match answered.payload {
            Some(response::Payload::Failure(failure)) => assert!(
                failure.reason.contains(named),
                "the refusal does not name {named}: {}",
                failure.reason
            ),
            other => panic!("a window onto {named} was asked for: {other:?}"),
        }
    }
    assert!(asked_for().is_empty(), "the shell was asked for a window");
}

/// The first window open onto the daemon's one tab, and a second opened beside it onto a tab it
/// asked for. Answers the two tabs, first window's first.
fn two_windows(daemon: &Daemon) -> (String, String) {
    start(daemon, "window-1");
    match answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))).payload {
        Some(response::Payload::Opened(opened)) => assert_eq!(opened.window, "window-1"),
        other => panic!("opening the first window answered {other:?}"),
    }
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
    match answer(&in_window(window, request::Payload::ReadWindow(ReadWindow::default()))).payload {
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
/// The machine whose tab a window is showing, from the last view sent to it.
fn machine_showing_in(window: &str) -> Option<String> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let view = events.iter().rev().find_map(|event| match &event.payload {
        Some(event::Payload::ViewChanged(view)) if event.window == window => Some(view),
        _ => None,
    })?;
    let region = view.regions.first()?;
    region.root.as_ref()?;
    Some(region.daemon_id.clone())
}

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

/// Asks for a new bridge for a pane on the test's one machine, as a problem's Reattach does.
fn reattach(pane: &str) -> request::Payload {
    request::Payload::ReattachPane(ReattachPane {
        daemon_id: String::new(),
        pane_id: pane.to_string(),
    })
}

/// How many bridges the last view sent to a window says it has replaced for a pane, or `None`
/// while that view does not draw the pane.
fn restarts_in(window: &str, pane: &str) -> Option<u32> {
    fn find(node: &ViewNode, pane: &str) -> Option<u32> {
        match &node.node {
            Some(view_node::Node::Pane(drawn)) => {
                (drawn.pane_id == pane).then_some(drawn.bridge_restarts)
            }
            Some(view_node::Node::Split(split)) => {
                split.first.iter().chain(split.second.iter()).find_map(|child| find(child, pane))
            }
            None => None,
        }
    }
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let view = events.iter().rev().find_map(|event| match &event.payload {
        Some(event::Payload::ViewChanged(view)) if event.window == window => Some(view),
        _ => None,
    })?;
    view.regions.iter().filter_map(|region| region.root.as_ref()).find_map(|root| find(root, pane))
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

/// Makes a tab from a window, which takes its keyboard, and names the pane in it.
fn new_tab_in(window: &str) -> String {
    let before = keyboard_in(window);
    assert_ok(&answer(&in_window(
        window,
        request::Payload::CreateTab(CreateTab { take_focus: true, ..CreateTab::default() }),
    )));
    until(
        &format!("{window}'s keyboard to move into the new tab"),
        || keyboard_in(window).is_some_and(|pane| Some(&pane) != before.as_ref()),
        || format!("{window}'s keyboard is on {:?}", keyboard_in(window)),
    );
    keyboard_in(window).expect("just waited for it")
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

/// What the next launch would be told to reopen, asked as a launch asks.
fn reopening(daemon: &Daemon) -> Vec<String> {
    let asked = Request::new(request::Payload::ReadReopening(ReadReopening {
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
    }));
    match answer(&asked).payload {
        Some(response::Payload::Reopening(reopening)) => reopening.arrangements,
        other => panic!("asking what to reopen answered {other:?}"),
    }
}

fn arrangement_text(daemon: &Daemon, window: &str) -> String {
    arrangement(daemon, window).to_string_lossy().into_owned()
}

/// How many views and rosters a window has been sent.
fn sent_to(window: &str) -> usize {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    events
        .iter()
        .filter(|event| event.window == window)
        .filter(|event| {
            matches!(
                event.payload,
                Some(event::Payload::ViewChanged(_) | event::Payload::RosterChanged(_))
            )
        })
        .count()
}

/// Every window the shell has been asked to open: its name, what to show, and whether fresh.
fn asked_for() -> Vec<(String, String, bool)> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    events
        .iter()
        .filter_map(|event| match &event.payload {
            Some(event::Payload::ReopenWindow(reopen)) => {
                Some((reopen.name.clone(), reopen.show.clone(), reopen.fresh))
            }
            _ => None,
        })
        .collect()
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
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
