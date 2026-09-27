//! A request about another window's tab is carried to that window (kan a_2Mhi0EZlv).
//!
//! Alex's rule: any `muster` verb works from any window. A tab belongs to exactly one window, so
//! the window a caller reached hands a request about another window's tab to that window, over
//! its command socket, and relays the answer. These drive this window's own command socket the
//! way the CLI does, with a stand-in for the other window that records what it was carried.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use herdr_harness::{Daemon, until};
use muster::proto::{
    AttentionChanged, Carried, CloseTab, CreateTab, Event, FocusPane, FocusTab, MoveTab,
    OpenWindow, ReadTabHolders, ReadWindow, RenameTab, ReopenWindow, Request, Response, Startup,
    event, request, response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, WindowName};
use muster_core::mirror::backend::TabId;
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use prost::Message;
use serde_json::json;

/// Going to, renaming and closing a tab another window holds all reach that window.
#[test]
fn a_request_about_another_windows_tab_is_carried_there() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");

    for request in [
        request::Payload::FocusTab(FocusTab { tab_id: theirs.clone(), ..FocusTab::default() }),
        request::Payload::RenameTab(RenameTab {
            tab_id: theirs.clone(),
            name: "renamed".to_string(),
            ..RenameTab::default()
        }),
        request::Payload::CloseTab(CloseTab { tab_id: theirs.clone(), ..CloseTab::default() }),
    ] {
        let answer = ask(&ours, request.clone());
        assert!(
            matches!(answer.payload, Some(response::Payload::Ok(_))),
            "the other window's answer was not relayed: {answer:?}"
        );
        let carried = other.last().expect("the other window was carried nothing");
        assert_eq!(carried.by, "window-1", "the carried request does not say who carried it");
        assert_eq!(
            carried.request.and_then(|request| request.payload),
            Some(request),
            "the other window was carried something other than what was asked"
        );
    }
    // Carried and not also done here: the stand-in closes nothing, so a tab gone from the daemon
    // would be this window acting on another window's tab after all.
    assert_eq!(daemon_tabs(&daemon).len(), 2, "the close was carried out here as well as carried");
}

/// A request carried to this window is answered here, and never carried on.
///
/// Two windows whose records briefly disagree would otherwise hand a request back and forth. And
/// going to a tab this way brings the window forward, because whoever asked was looking at
/// something else.
#[test]
fn a_request_carried_here_is_answered_here() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");
    RAISED.lock().expect("a panicking test poisoned the log").clear();

    // Carried here although the record says the other window has it: this window answers, and
    // refuses rather than showing another window's tab.
    let answer = ask(
        &ours,
        request::Payload::Carried(Box::new(Carried {
            by: "window-9".to_string(),
            request: Some(Box::new(Request {
                payload: Some(request::Payload::FocusTab(FocusTab {
                    tab_id: theirs,
                    ..FocusTab::default()
                })),
            })),
        })),
    );
    assert!(other.last().is_none(), "a carried request was carried on");
    assert!(
        matches!(answer.payload, Some(response::Payload::Failure(_))),
        "this window went to a tab it does not hold: {answer:?}"
    );

    // And one of its own tabs, carried here, is gone to and brings the window forward.
    let own = listed().first().cloned().expect("the window holds a tab");
    let answer = ask(
        &ours,
        request::Payload::Carried(Box::new(Carried {
            by: "window-9".to_string(),
            request: Some(Box::new(Request {
                payload: Some(request::Payload::FocusTab(FocusTab {
                    tab_id: own,
                    ..FocusTab::default()
                })),
            })),
        })),
    );
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert!(
        RAISED.lock().expect("a panicking test poisoned the log").contains(&0),
        "going to a tab from another window left this window behind whatever was in front"
    );
}

/// Going to a pane in another open window's tab from this window's own shell reaches that window.
///
/// A notification is the one way the shell asks about a tab it does not list: macOS hands the click
/// to whichever instance it chooses, and the pane may be in any window. The shell calls in on its
/// main thread, which never waits on another window, so the request is carried from a thread of
/// its own and the call answers at once.
#[test]
fn a_notification_click_for_another_windows_pane_reaches_that_window() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");
    let (pane, _) = pane_of(&daemon, &theirs);
    RAISED.lock().expect("a panicking test poisoned the log").clear();

    let focused = answer(request::Payload::FocusPane(FocusPane {
        pane_id: pane.clone(),
        ..FocusPane::default()
    }));

    assert!(
        matches!(focused.payload, Some(response::Payload::Ok(_))),
        "going to {pane} in another window's tab was refused: {focused:?}"
    );
    until(
        "the other window to be carried the focus",
        || other.carried.lock().expect("a panicking test poisoned the log").len() == 1,
        || "the other window was carried nothing".to_string(),
    );
    let carried = other.last().expect("just waited for it");
    assert_eq!(
        carried.request.and_then(|request| request.payload),
        Some(request::Payload::FocusPane(FocusPane { pane_id: pane, ..FocusPane::default() })),
        "the other window was carried something other than the focus"
    );
    assert!(!listed().contains(&theirs), "the other window's tab was brought here instead");
    // The stand-in's pid, which is what this window hands activation to: on macOS 14 an app comes
    // forward only when the active one lets it.
    assert_eq!(
        RAISED.lock().expect("a panicking test poisoned the log").clone(),
        vec![1],
        "this window did not hand activation to the window it carried the focus to"
    );
}

/// A tab moved to a window by name joins that window's list, whichever window was asked.
///
/// From outside a pane, the CLI sends a move naming its window to whichever window answers first.
/// The outcome deciding on who answered meant `--window window-1` brought the tab on screen when
/// window-1 answered and only listed it when another window did. Only a move naming no window -
/// "bring it here" - is somebody looking at the window it lands in.
#[test]
fn a_tab_moved_to_a_window_by_name_joins_its_list_without_coming_on_screen() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let own = listed().first().cloned().expect("the window opened onto a tab");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");

    let answer = ask(&ours, move_tab(&theirs, "window-1"));

    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert!(listed().contains(&theirs), "the tab moved here is not listed: {:?}", listed());
    assert_eq!(holder(&daemon, &theirs).as_deref(), Some("window-1"));
    assert_eq!(
        showing().as_deref(),
        Some(own.as_str()),
        "a tab moved here by name came on screen, which a move asked of another window would not"
    );
}

/// A tab whose window is closed is closed from whichever window was asked.
///
/// The daemon does the closing, and the closed window only remembers the tab. Refusing would
/// leave a tab nothing could close until its window was reopened.
#[test]
fn a_closed_windows_tab_is_closed_from_here() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-8");
    let before = daemon_tabs(&daemon).len();

    let answer =
        ask(&ours, request::Payload::CloseTab(CloseTab { tab_id: theirs, ..CloseTab::default() }));
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    until(
        "the daemon to close the closed window's tab",
        || daemon_tabs(&daemon).len() == before - 1,
        || format!("the daemon still holds {:?}", daemon_tabs(&daemon)),
    );
}

/// A tab moved into this window comes on screen, and one moved out leaves, with every pane in
/// both still running.
///
/// The three doors - `muster tab move`, the menu, a dropped row - all send this one request.
#[test]
fn a_tab_moves_between_windows_and_its_panes_keep_running() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");
    let panes = daemon_panes(&daemon);

    // Pulled here, by naming no window.
    let answer = ask(&ours, move_tab(&theirs, ""));
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert!(listed().contains(&theirs), "the tab moved here is not listed: {:?}", listed());
    until(
        "the tab moved here to come on screen",
        || showing().as_deref() == Some(theirs.as_str()),
        || format!("the window shows {:?}", showing()),
    );

    // Sent away, by the pid `muster window` prints for the other window.
    let answer = ask(&ours, move_tab(&theirs, "1"));
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert!(!listed().contains(&theirs), "the tab sent away is still listed: {:?}", listed());
    assert_eq!(holder(&daemon, &theirs).as_deref(), Some("window-9"));

    assert_eq!(daemon_panes(&daemon), panes, "moving tabs between windows ended a pane");
}

/// A closed window can be given a tab, by name, and it is there when that window reopens.
#[test]
fn a_tab_moves_to_a_closed_window_by_name() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let ours = open_a_window(&daemon, "window-1");
    let _ = a_second_tab_given_to(&daemon, &ours, "window-8");
    let own = listed().first().cloned().expect("the window holds a tab");

    for (window, why) in
        [("window-77", "no window is called"), ("999999", "no open window has pid")]
    {
        let answer = ask(&ours, move_tab(&own, window));
        match answer.payload {
            Some(response::Payload::Failure(failure)) => assert!(
                failure.reason.contains(why),
                "moving to {window} was refused without saying why: {}",
                failure.reason
            ),
            other => panic!("a tab was moved to {window}, which is not a window: {other:?}"),
        }
    }

    let answer = ask(&ours, move_tab(&own, "window-8"));
    assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    assert_eq!(holder(&daemon, &own).as_deref(), Some("window-8"));
    assert!(listed().is_empty(), "the tab given to a closed window is still listed here");
}

/// `muster window` says which tabs every other window holds, open or closed.
///
/// This window lists only its own now, and any verb works from any window - so a caller needs
/// somewhere to find a name another window holds, and a closed window's running agents need
/// somewhere to be seen.
#[test]
fn the_window_says_what_every_other_window_holds() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");

    let window = match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    };
    assert_eq!(window.name, "window-1");
    let other = window
        .windows
        .iter()
        .find(|other| other.name == "window-9")
        .unwrap_or_else(|| panic!("the other window is not listed: {:?}", window.windows));
    assert_eq!(other.pid, 1, "an open window is not given its pid");
    assert_eq!(
        other.tabs.iter().map(|tab| tab.tab_id.clone()).collect::<Vec<_>>(),
        vec![theirs.clone()],
        "the other window's tab is not listed under it"
    );
    assert!(
        other.tabs.iter().all(|tab| tab.place == 0 && tab.panes.iter().all(|pane| pane.place == 0)),
        "another window's tabs carry numbers this window made up: {:?}",
        other.tabs
    );
}

/// Going to a closed window's tab asks for that window to be opened again, onto it.
///
/// A closed window keeps its tabs and its agents keep running, so going to one of them - from a
/// notification, or `muster tab focus` - is going to that window. Opening one is starting an app,
/// which is the shell's, so the core asks.
#[test]
fn going_to_a_closed_windows_tab_reopens_that_window() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-8");
    REOPENED.lock().expect("a panicking test poisoned the log").clear();

    // Twice, as a double click or a click and a command close together would: the window is
    // still starting when the second arrives, and a second launch would open a different window.
    for _ in 0..2 {
        let answer = ask(
            &ours,
            request::Payload::FocusTab(FocusTab { tab_id: theirs.clone(), ..FocusTab::default() }),
        );
        assert!(matches!(answer.payload, Some(response::Payload::Ok(_))), "{answer:?}");
    }
    let reopened = REOPENED.lock().expect("a panicking test poisoned the log").clone();
    assert_eq!(
        reopened.iter().map(|asked| (asked.name.as_str(), asked.show.as_str())).collect::<Vec<_>>(),
        vec![("window-8", theirs.as_str())],
        "going to a closed window's tab did not ask for that window back exactly once"
    );
    assert!(!listed().contains(&theirs), "the closed window's tab was brought here instead");
}

/// A blocked agent in a closed window's tab is announced by the window in front, and one in an
/// open window's tab is left to that window.
///
/// A closed window cannot say anything, and its agents are still running - so without this a
/// blocked agent there would reach nobody. An open window speaks for itself, and two windows
/// posting one agent is noise.
#[test]
fn a_closed_windows_blocked_agent_is_announced_here_and_an_open_ones_is_not() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let closed = a_second_tab_given_to(&daemon, &ours, "window-8");
    let open = a_second_tab_given_to(&daemon, &ours, "window-9");
    ASKED.lock().expect("a panicking test poisoned the log").clear();

    for (tab, announced) in [(&closed, true), (&open, false)] {
        let (muster, backend) = pane_of(&daemon, tab);
        daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": backend, "agent": "probe", "source": "probe", "state": "blocked" }),
        );
        until(
            "this window to hear the agent is blocked",
            || state_of(&muster).as_deref() == Some("blocked"),
            || format!("the window says {:?}", state_of(&muster)),
        );
        let asked = ASKED
            .lock()
            .expect("a panicking test poisoned the log")
            .iter()
            .any(|asked| asked.pane_id == muster && asked.state == "blocked");
        assert_eq!(
            asked,
            announced,
            "a blocked agent in {tab} was {} announced here",
            if asked { "" } else { "not" }
        );
    }
}

/// A blocked agent in a tab nobody holds is announced by the window in front, and by no other.
///
/// A tab made outside Muster is held by nobody until the window in front takes it, and every
/// window hears its agents meanwhile. Each of them announcing one is a notification per window.
#[test]
fn a_tab_nobody_holds_is_announced_only_by_the_window_in_front() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let _other = Stand::in_for(&daemon, "window-9");
    open_a_window(&daemon, "window-1");
    let own = listed().first().cloned().expect("the window opened onto a tab");
    let path = record(&daemon);
    let mut holders = read_record(&path);
    holders.focused(&WindowName::new("window-9"), i64::MAX / 2);
    write_record(&path, &holders);
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));

    // The stand-in in front takes nothing, so this tab stays held by nobody.
    daemon.call("tab.create", &json!({ "focus": false }));
    until(
        "this window to hear of the tab nobody asked for",
        || all_panes().len() == 2,
        || format!("the window has heard of {:?}", all_panes()),
    );
    let ours = panes_listed_in(&own);
    let nobodys = all_panes().into_iter().find(|pane| !ours.contains(pane)).expect("two panes");
    ASKED.lock().expect("a panicking test poisoned the log").clear();

    // Nobody's first, then this window's own: the daemon announces them in that order, so once
    // the second is announced here the first has been decided.
    for pane in [&nobodys, &ours[0]] {
        let backend = backend_of(&daemon, pane);
        daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": backend, "agent": "probe", "source": "probe", "state": "blocked" }),
        );
    }
    until(
        "this window to announce its own blocked agent",
        || announced().contains(&ours[0]),
        || format!("this window announced {:?}", announced()),
    );
    assert!(
        !announced().contains(&nobodys),
        "a window behind the one in front announced an agent in a tab nobody holds"
    );
}

fn announced() -> Vec<String> {
    ASKED
        .lock()
        .expect("a panicking test poisoned the log")
        .iter()
        .filter(|asked| asked.state == "blocked")
        .map(|asked| asked.pane_id.clone())
        .collect()
}

/// Every pane this window has heard of, in any tab.
fn all_panes() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.iter().map(|held| held.pane_id.clone()).collect()
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn panes_listed_in(tab: &str) -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .filter(|held| held.tab_id == tab)
            .flat_map(|held| held.panes.iter())
            .map(|pane| pane.pane_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The daemon's own name for a pane, through the record both sides write names into.
fn backend_of(daemon: &Daemon, muster: &str) -> String {
    let names = std::fs::read_to_string(daemon.root().join("panes.toml"))
        .expect("the window wrote its names");
    let (panes, _) = muster_core::names::from_toml(&names, muster_core::names::Mint::Drawn)
        .expect("the names read back");
    panes.locate(&muster_core::mirror::backend::PaneId::new(muster)).map_or_else(
        || panic!("{muster} has no backend name in the record"),
        |located| located.backend.to_string(),
    )
}

/// A window opened to show something shows it.
///
/// What a closed window is reopened with when somebody went to one of its tabs from elsewhere.
#[test]
fn a_window_opened_to_show_a_tab_shows_it() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let ours = open_a_window(&daemon, "window-1");
    let first = listed().first().cloned().expect("the window holds a tab");
    let _ = a_second_tab_given_to(&daemon, &ours, "window-8");
    let moved = ask(&ours, move_tab(&first, "window-8"));
    assert!(matches!(moved.payload, Some(response::Payload::Ok(_))), "{moved:?}");
    assert_ok(&answer(request::Payload::Quitting(muster::proto::Quitting::default())));

    turn.relaunch();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: daemon.root().join("window-8.toml").to_string_lossy().into_owned(),
        pane_names_path: daemon.root().join("panes.toml").to_string_lossy().into_owned(),
        tab_holders_path: record(&daemon).to_string_lossy().into_owned(),
        show: first.clone(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the reopened window to show the tab it was opened for",
        || showing().as_deref() == Some(first.as_str()),
        || format!("the window shows {:?} and lists {:?}", showing(), listed()),
    );
}

/// A pane in a tab, by Muster's name and by the daemon's.
fn pane_of(daemon: &Daemon, tab: &str) -> (String, String) {
    let window = match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    };
    let muster = window
        .windows
        .iter()
        .flat_map(|other| other.tabs.iter())
        .find(|held| held.tab_id == tab)
        .and_then(|held| held.panes.first())
        .map_or_else(
            || panic!("no other window lists {tab}: {:?}", window.windows),
            |pane| pane.pane_id.clone(),
        );
    // The daemon's own name for it, through the record both sides write names into.
    let names = std::fs::read_to_string(daemon.root().join("panes.toml"))
        .expect("the window wrote its names");
    let (panes, _) = muster_core::names::from_toml(&names, muster_core::names::Mint::Drawn)
        .expect("the names read back");
    let backend = panes.locate(&muster_core::mirror::backend::PaneId::new(&muster)).map_or_else(
        || panic!("{muster} has no backend name in the record"),
        |located| located.backend.to_string(),
    );
    (muster, backend)
}

fn state_of(pane: &str) -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.iter().find(|held| held.pane_id == pane).map(|held| held.state.clone())
        }
        _ => None,
    }
}

fn move_tab(tab: &str, window: &str) -> request::Payload {
    request::Payload::MoveTab(MoveTab { tab_id: tab.to_string(), window: window.to_string() })
}

fn holder(daemon: &Daemon, tab: &str) -> Option<String> {
    read_record(&record(daemon)).holder(&TabId::new(tab)).map(ToString::to_string)
}

fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let mut panes: Vec<String> = snapshot["snapshot"]["panes"]
        .as_array()
        .map(|panes| {
            panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default();
    panes.sort();
    panes
}

fn showing() -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => {
            window.view.and_then(|view| view.regions.first().map(|region| region.tab_id.clone()))
        }
        _ => None,
    }
}

/// The other window, as far as this one can tell: a socket that answers, named in the record.
struct Stand {
    carried: Arc<Mutex<Vec<Carried>>>,
}

impl Stand {
    fn in_for(daemon: &Daemon, name: &str) -> Stand {
        let socket = daemon.root().join(format!("{name}.sock"));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("a socket can be bound");
        let arrangement = daemon.root().join(format!("{name}.toml"));
        std::fs::write(&arrangement, "").expect("a stand-in arrangement can be written");
        let path = record(daemon);
        let mut holders = read_record(&path);
        holders.opened(HeldWindow {
            name: WindowName::new(name),
            arrangement: arrangement.to_string_lossy().into_owned(),
            socket: socket.to_string_lossy().into_owned(),
            pid: 1,
            focused: 0,
            daemons: std::iter::once(DaemonId::new("local")).collect(),
        });
        write_record(&path, &holders);

        let carried = Arc::new(Mutex::new(Vec::new()));
        let noted = Arc::clone(&carried);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                // A window asking whether this one is open connects and says nothing.
                let Ok(bytes) = read_frame(&mut stream, LARGEST_MESSAGE) else { continue };
                if let Ok(Request { payload: Some(request::Payload::Carried(carried)) }) =
                    Request::decode(bytes.as_slice())
                {
                    noted.lock().expect("a panicking test poisoned the log").push(*carried);
                }
                let _ = write_frame(&mut stream, &Response::ok().encode_to_vec());
            }
        });
        Stand { carried }
    }

    fn last(&self) -> Option<Carried> {
        self.carried.lock().expect("a panicking test poisoned the log").pop()
    }
}

/// Makes a second tab in this window and gives it to another one, as that window taking it would.
fn a_second_tab_given_to(daemon: &Daemon, ours: &Path, window: &str) -> String {
    assert!(matches!(
        ask(
            ours,
            request::Payload::CreateTab(CreateTab { take_focus: true, ..CreateTab::default() })
        )
        .payload,
        Some(response::Payload::Made(_) | response::Payload::Ok(_))
    ));
    // The pane as well as the tab: herdr can announce a tab before the pane in it, and callers
    // go on to name that pane.
    until(
        "the second tab and its pane to arrive",
        || listed().len() == 2 && listed().last().is_some_and(|tab| has_a_pane(tab)),
        || format!("this window lists {:?}", listed()),
    );
    let theirs = listed().last().cloned().expect("just waited for it");
    let path = record(daemon);
    let mut holders = read_record(&path);
    holders.take(TabId::new(&theirs), &WindowName::new(window));
    write_record(&path, &holders);
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));
    assert_eq!(listed().len(), 1, "the tab given away is still listed here");
    theirs
}

fn open_a_window(daemon: &Daemon, name: &str) -> PathBuf {
    muster::ffi::muster_set_event_callback(Some(note));
    let socket = daemon.root().join(format!("{name}.sock"));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: daemon.root().join(format!("{name}.toml")).to_string_lossy().into_owned(),
        pane_names_path: daemon.root().join("panes.toml").to_string_lossy().into_owned(),
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a tab with a pane in it",
        || listed().first().is_some_and(|tab| has_a_pane(tab)),
        || format!("the window lists {:?}", listed()),
    );
    socket
}

fn has_a_pane(tab: &str) -> bool {
    !panes_listed_in(tab).is_empty()
}

/// Asks this window over its command socket, the way the CLI does.
fn ask(socket: &Path, payload: request::Payload) -> Response {
    let mut stream = UnixStream::connect(socket).expect("the window is listening");
    write_frame(&mut stream, &Request { payload: Some(payload) }.encode_to_vec())
        .expect("the request can be written");
    let bytes = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers");
    Response::decode(bytes.as_slice()).expect("the window answers with a response")
}

fn daemon_tabs(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["tabs"]
        .as_array()
        .map(|tabs| {
            tabs.iter().filter_map(|tab| tab["tab_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default()
}

fn record(daemon: &Daemon) -> PathBuf {
    daemon.root().join("holding/tabs.toml")
}

fn read_record(path: &Path) -> Holders {
    from_toml(&std::fs::read_to_string(path).unwrap_or_default())
        .expect("the record this window writes reads back")
}

fn write_record(path: &Path, holders: &Holders) {
    std::fs::create_dir_all(path.parent().expect("the record is in a directory"))
        .expect("the record's directory can be made");
    std::fs::write(path, to_toml(holders)).expect("the record can be written");
}

fn listed() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .map(|tab| tab.tab_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

/// The `pid` of every `RaiseWindow` emitted, 0 for this window itself.
static RAISED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static REOPENED: Mutex<Vec<ReopenWindow>> = Mutex::new(Vec::new());
static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::RaiseWindow(raise)) => {
            RAISED.lock().expect("a panicking test poisoned the log").push(raise.pid);
        }
        Some(event::Payload::ReopenWindow(reopen)) => {
            REOPENED.lock().expect("a panicking test poisoned the log").push(reopen);
        }
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        _ => {}
    }
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
