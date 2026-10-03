//! A daemon slow to answer does not hold the window closed.
//!
//! Every daemon a config names used to be attached, one after another, before the shell had made
//! a window, so a devenv slow to answer kept the window from appearing for up to a minute and a
//! half (kan a_2Y3LSOogS). Staged with a relay in front of a real daemon that holds back its
//! answer to the window's subscribe (`muster_harness::Relay`), because nothing can ask a daemon
//! to be slow on cue. Local rather than over ssh, because the rule is the same for every daemon
//! and the transport adds nothing to it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use std::path::Path;

use muster::proto::{
    CreateTab, Event, OpenWindow, ProblemsChanged, Quitting, ReadWindow, Request, Response,
    Startup, event, request, response,
};
use muster_core::composition::{DaemonId, saved};
use muster_core::mirror::{PaneId, TabId};
use muster_daemon_proto::{self as daemon_proto, session_request};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// How long the relay holds back the daemon's state: longer than anyone would wait for a
/// window, shorter than the window's own patience for a first snapshot.
const SLOW: Duration = Duration::from_secs(5);

#[test]
fn a_slow_daemon_does_not_hold_the_window_closed() {
    let _turn = muster::testing::fresh_session();
    watch_problems();
    let daemon = Daemon::start_built();
    let relay = daemon.delaying_answers_where(subscribes, SLOW);

    let asked = Instant::now();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: relay.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    let started_in = asked.elapsed();
    assert!(
        started_in < Duration::from_secs(3),
        "starting took {started_in:?} waiting for a daemon {SLOW:?} slow to answer.\n  Impact: \
         the window does not appear until every daemon it names has answered."
    );

    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    assert_eq!(
        health_of("local").first().map(String::as_str),
        Some("connecting"),
        "the window was not told what it is waiting for"
    );
    until(
        "the slow daemon's pane to arrive in the open window",
        || listed_panes() == 1,
        || format!("the window lists {} panes", listed_panes()),
    );
    until(
        "the window to be told the daemon is connected",
        || health_of("local").last().map(String::as_str) == Some("connected"),
        || format!("the window was told {:?}", health_of("local")),
    );
    drop(relay);
}

/// A daemon that was not there when the window opened is attached once it is, without a relaunch,
/// and the window says meanwhile that it is missing.
///
/// A relaunch is what a daemon given up on at launch used to need, and it costs the panes on
/// every machine that was fine. Nothing listens at the socket the config names until a real
/// daemon's socket is linked there, which is a daemon coming up from the window's point of view.
#[test]
fn a_daemon_missing_at_launch_is_attached_once_it_answers() {
    let _turn = muster::testing::fresh_session();
    watch_problems();
    let daemon = Daemon::start_built();
    let named = daemon.root().join("named.sock");
    let config = daemon.root().join("muster.toml");
    std::fs::write(
        &config,
        format!("[[daemon]]\nid = \"local\"\nsocket = {:?}\n", named.to_string_lossy()),
    )
    .expect("the harness root is writable");

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let named_path = named.to_string_lossy().to_string();
    until(
        "the window to say the daemon is missing",
        || problems().iter().any(|problem| problem.contains(&named_path)),
        || format!("the problems raised are {:?}", problems()),
    );

    std::os::unix::fs::symlink(daemon.socket_path(), &named).expect("the socket can be linked");
    until(
        "the daemon's pane to arrive once it answers",
        || listed_panes() == 1,
        || format!("the window lists {} panes", listed_panes()),
    );
    until(
        "the window to take back what it said",
        || problems().is_empty(),
        || format!("the problems still raised are {:?}", problems()),
    );
}

/// A daemon on its way when the window opens keeps its place in the arrangement, and gets it
/// back when it answers.
///
/// The window writes its arrangement as it opens. Written as it stood then, without the tabs of a
/// daemon that had not answered, one slow launch erased them from the file, and a daemon that
/// answered later had its tabs opened at the end of the list.
#[test]
fn a_daemon_on_its_way_keeps_its_place_in_the_arrangement() {
    let turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let arrangement = daemon.root().join("window-1.toml");
    start(&daemon.muster_config(), &arrangement);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to open onto a tab",
        || listed_tabs().len() == 1,
        || format!("the window lists {:?}", listed_tabs()),
    );
    assert_ok(&answer(request::Payload::CreateTab(CreateTab {
        take_focus: true,
        ..CreateTab::default()
    })));
    until(
        "the second tab to arrive",
        || listed_tabs().len() == 2,
        || format!("the window lists {:?}", listed_tabs()),
    );
    let before = listed_tabs();
    assert_ok(&answer(request::Payload::Quitting(Quitting::default())));

    turn.relaunch();
    let relay = daemon.delaying_answers_where(subscribes, SLOW);
    start(&relay.muster_config(), &arrangement);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    assert_eq!(
        saved_tabs(&arrangement),
        before,
        "opening while the daemon was on its way rewrote the arrangement without its tabs"
    );
    until(
        "the daemon's tabs to come back as they were left",
        || listed_tabs() == before,
        || format!("the window lists {:?} and was left with {before:?}", listed_tabs()),
    );
    drop(relay);
}

#[test]
fn a_daemon_on_its_way_takes_neither_the_screen_nor_the_keyboard() {
    // Somebody typing into the tab on screen when a devenv answers goes on typing there. Its
    // tabs coming back used to bring the last of them on screen with the keyboard in the
    // devenv's half, so the next keystrokes reached an agent on another machine.
    let _turn = muster::testing::fresh_session();
    let local = Daemon::start_built();
    make(&mut local.connect(), create("a1", in_new_tab("t1")));
    make(&mut local.connect(), create("a3", in_new_tab("t3")));
    let devenv = Daemon::start_built();
    make(&mut devenv.connect(), create("b2", in_new_tab("t2")));
    make(&mut devenv.connect(), create("b3", in_new_tab("t3")));
    let relay = devenv.delaying_answers_where(subscribes, SLOW);

    let region = |daemon: &str, pane: &str, keyboard: bool| saved::SavedRegion {
        daemon: DaemonId::new(daemon),
        weight: 1.0,
        pane: Some(PaneId::new(pane)),
        keyboard,
    };
    let tab = |id: &str, regions| saved::SavedTab { id: TabId::new(id), regions };
    let arrangement = local.root().join("window-1.toml");
    let left = saved::Saved {
        tabs: vec![
            tab("t1", vec![region("local", "a1", true)]),
            tab("t2", vec![region("devenv", "b2", true)]),
            tab("t3", vec![region("local", "a3", true), region("devenv", "b3", false)]),
        ],
        showing: Some(TabId::new("t1")),
        ..saved::Saved::default()
    };
    std::fs::write(&arrangement, saved::to_toml(&left)).expect("the harness root is writable");
    let config = local.root().join("muster-late.toml");
    std::fs::write(
        &config,
        format!(
            "[[daemon]]\nid = \"local\"\nsocket = {:?}\n\n[[daemon]]\nid = \"devenv\"\nsocket = \
             {:?}\n",
            local.socket_path().to_string_lossy(),
            relay.socket_path().to_string_lossy()
        ),
    )
    .expect("the harness root is writable");

    start(&config, &arrangement);
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    assert_eq!(keyboard(), ("t1".to_string(), "a1".to_string()), "the window opened elsewhere");
    until(
        "the devenv's tabs to come back in their place",
        || listed_tabs() == ["t1", "t2", "t3"],
        || format!("the window lists {:?}", listed_tabs()),
    );
    assert_eq!(
        keyboard(),
        ("t1".to_string(), "a1".to_string()),
        "the devenv answering moved the screen or the keyboard"
    );
    drop(relay);
}

/// The tab on screen and the pane the keyboard is in.
fn keyboard() -> (String, String) {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return (String::new(), String::new());
    };
    let view = window.view.unwrap_or_default();
    let pane = view
        .regions
        .iter()
        .find(|region| region.region_id == view.focused_region)
        .map(|region| region.pane_id.clone())
        .unwrap_or_default();
    (view.tab_id, pane)
}

fn start(config: &Path, arrangement: &Path) {
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
}

/// The tabs the window lists, in order.
fn listed_tabs() -> Vec<String> {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .map(|tab| tab.tab_id.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// The tabs the arrangement on disk names, in order.
fn saved_tabs(arrangement: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(arrangement).unwrap_or_default();
    saved::from_toml(&text)
        .map(|saved| saved.tabs.iter().map(|tab| tab.id.to_string()).collect())
        .unwrap_or_default()
}

static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);
static HEALTH: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Every health the window was told about one daemon, in order.
fn health_of(daemon: &str) -> Vec<String> {
    HEALTH
        .lock()
        .expect("a panicking test poisoned the health")
        .iter()
        .filter(|(said_of, _)| said_of == daemon)
        .map(|(_, state)| state.clone())
        .collect()
}

/// Throws away what the last test heard and listens again: problems and daemon health.
fn watch_problems() {
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    HEALTH.lock().expect("a panicking test poisoned the health").clear();
    muster::ffi::muster_set_event_callback(Some(note));
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ProblemsChanged(problems)) => {
            *PROBLEMS.lock().expect("a panicking test poisoned the problems") = Some(problems);
        }
        Some(event::Payload::BackendHealth(health)) => {
            HEALTH
                .lock()
                .expect("a panicking test poisoned the health")
                .push((health.daemon_id, health.state));
        }
        _ => {}
    }
}

/// What each problem the window has raised says.
fn problems() -> Vec<String> {
    PROBLEMS
        .lock()
        .expect("a panicking test poisoned the problems")
        .clone()
        .map(|changed| changed.problems.into_iter().map(|problem| problem.detail).collect())
        .unwrap_or_default()
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

fn listed_panes() -> usize {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window.panes.len(),
        _ => 0,
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
