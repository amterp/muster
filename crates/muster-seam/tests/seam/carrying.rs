//! Another window's tabs, as the record of which window holds each tab says (kan a_2Mhi0EZlv).
//!
//! Alex's rule: any `muster` verb works from any window, and a tab belongs to exactly one window.
//! These drive this window's own command socket the way the CLI does, with closed windows and a
//! stand-in for an open window in another process written into the record.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use muster::proto::{
    AttentionChanged, CloseTab, CreateTab, Event, FocusTab, MoveTab, OpenWindow, ReadTabHolders,
    ReadWindow, ReopenWindow, Request, Response, Startup, event, request, response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, WindowName};
use muster_core::mirror::backend::TabId;
use muster_daemon_proto::AgentState;
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{Daemon, until};
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use prost::Message;

/// A tab moved to a window by name joins that window's list, whichever window was asked.
///
/// From outside a pane, the CLI sends a move naming its window to whichever window answers first.
/// The outcome deciding on who answered meant `--window window-1` brought the tab on screen when
/// window-1 answered and only listed it when another window did. Only a move naming no window -
/// "bring it here" - is somebody looking at the window it lands in.
#[test]
fn a_tab_moved_to_a_window_by_name_joins_its_list_without_coming_on_screen() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
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
    let daemon = Daemon::start_built();
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
    let daemon = Daemon::start_built();
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
    let daemon = Daemon::start_built();
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
    let daemon = Daemon::start_built();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let theirs = a_second_tab_given_to(&daemon, &ours, "window-9");

    let window = match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
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
    let daemon = Daemon::start_built();
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
    let daemon = Daemon::start_detecting();
    let _other = Stand::in_for(&daemon, "window-9");
    let ours = open_a_window(&daemon, "window-1");
    let closed = a_second_tab_given_to(&daemon, &ours, "window-8");
    let open = a_second_tab_given_to(&daemon, &ours, "window-9");
    let panes = [(pane_of(&closed), true), (pane_of(&open), false)];
    for (pane, _) in &panes {
        daemon.run_agent(pane);
    }
    ASKED.lock().expect("a panicking test poisoned the log").clear();

    for (pane, announced) in panes {
        daemon.set_agent_state(&pane, AgentState::Blocked);
        until(
            "this window to hear the agent is blocked",
            || state_of(&pane).as_deref() == Some("blocked"),
            || format!("the window says {:?}", state_of(&pane)),
        );
        let asked = ASKED
            .lock()
            .expect("a panicking test poisoned the log")
            .iter()
            .any(|asked| asked.pane_id == pane && asked.state == "blocked");
        assert_eq!(
            asked,
            announced,
            "a blocked agent in {pane} was {} announced here",
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
    let daemon = Daemon::start_detecting();
    let _other = Stand::in_for(&daemon, "window-9");
    open_a_window(&daemon, "window-1");
    let own = listed().first().cloned().expect("the window opened onto a tab");
    let path = record(&daemon);
    let mut holders = read_record(&path);
    holders.focused(&WindowName::new("window-9"), i64::MAX / 2);
    write_record(&path, &holders);
    assert_ok(&answer(request::Payload::ReadTabHolders(ReadTabHolders {})));

    // Made on the daemon rather than through a window, and the stand-in in front takes nothing,
    // so this tab stays held by nobody.
    make(&mut daemon.connect(), create("p1nobody00", in_new_tab("t1nobody00")));
    until(
        "this window to hear of the tab nobody asked for",
        || all_panes().len() == 2,
        || format!("the window has heard of {:?}", all_panes()),
    );
    let ours = panes_listed_in(&own);
    let nobodys = all_panes().into_iter().find(|pane| !ours.contains(pane)).expect("two panes");
    for pane in [&nobodys, &ours[0]] {
        daemon.run_agent(pane);
    }
    ASKED.lock().expect("a panicking test poisoned the log").clear();

    // Nobody's first, then this window's own: the daemon announces them in that order, so once
    // the second is announced here the first has been decided.
    for pane in [&nobodys, &ours[0]] {
        daemon.set_agent_state(pane, AgentState::Blocked);
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
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            window.panes.iter().map(|held| held.pane_id.clone()).collect()
        }
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn panes_listed_in(tab: &str) -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
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

/// A window opened to show something shows it.
///
/// What a closed window is reopened with when somebody went to one of its tabs from elsewhere.
#[test]
fn a_window_opened_to_show_a_tab_shows_it() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
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
        tab_holders_path: record(&daemon).to_string_lossy().into_owned(),
        show: first.clone(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the reopened window to show the tab it was opened for",
        || showing().as_deref() == Some(first.as_str()),
        || format!("the window shows {:?} and lists {:?}", showing(), listed()),
    );
}

/// The first pane in a tab another window holds.
fn pane_of(tab: &str) -> String {
    let window = match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    };
    window
        .windows
        .iter()
        .flat_map(|other| other.tabs.iter())
        .find(|held| held.tab_id == tab)
        .and_then(|held| held.panes.first())
        .map_or_else(
            || panic!("no other window lists {tab}: {:?}", window.windows),
            |pane| pane.pane_id.clone(),
        )
}

fn state_of(pane: &str) -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
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
    let mut panes: Vec<String> =
        snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect();
    panes.sort();
    panes
}

fn showing() -> Option<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => {
            window.view.and_then(|view| view.regions.first().map(|region| region.tab_id.clone()))
        }
        _ => None,
    }
}

/// The other window, as far as this one can tell: a socket that answers, named in the record, as
/// a window process from before one app per install was.
struct Stand;

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
            install: String::new(),
            focused: 0,
            daemons: std::iter::once(DaemonId::new("local")).collect(),
        });
        write_record(&path, &holders);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                // A window asking whether this one is open connects and says nothing.
                if read_frame(&mut stream, LARGEST_MESSAGE).is_err() {
                    continue;
                }
                let _ = write_frame(&mut stream, &Response::ok().encode_to_vec());
            }
        });
        Stand
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
    // A submit returns with its effect already in the window, so the tab and the pane in it -
    // which callers go on to name - are listed by the time the request is answered.
    assert!(
        listed().len() == 2 && listed().last().is_some_and(|tab| has_a_pane(tab)),
        "a new tab and its pane were not listed when the request making them was answered: {:?}",
        listed()
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
        tab_holders_path: record(daemon).to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
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
    write_frame(&mut stream, &Request::new(payload).encode_to_vec())
        .expect("the request can be written");
    let bytes = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers");
    Response::decode(bytes.as_slice()).expect("the window answers with a response")
}

fn daemon_tabs(daemon: &Daemon) -> Vec<String> {
    snapshot(&mut daemon.connect()).tabs.into_iter().map(|tab| tab.tab).collect()
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
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .map(|tab| tab.tab_id.clone())
            .collect(),
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

static REOPENED: Mutex<Vec<ReopenWindow>> = Mutex::new(Vec::new());
static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
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
