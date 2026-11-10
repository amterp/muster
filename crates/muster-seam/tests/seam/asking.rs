//! One keystroke to the pane that asked: whichever pane is most urgently asking for somebody,
//! brought on screen with the keyboard on it, wherever it is in the window.

use muster::proto::{
    AskingListChanged, AttentionChanged, Event, FocusAsking, OpenWindow, ReadAsking, ReadWindow,
    RenamePane, Request, Response, Startup, WindowFocus, event, request, response,
};
use muster_daemon_proto::{AgentState, input_event};
use muster_harness::requests::{create, in_new_tab, make, until_text};
use muster_harness::{Daemon, Input, until};
use prost::Message;
use std::sync::Mutex;

/// A program asks from a tab nobody is showing. Going to the pane that asked shows that tab with
/// the keyboard on the pane, and once somebody has looked there is nothing left to go to.
#[test]
fn going_to_the_pane_that_asked_shows_it_and_then_nothing_is_left() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    TOLD.lock().expect("a panicking test poisoned the log").clear();
    LISTS.lock().expect("asking lists").clear();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", in_new_tab("t2")));
    until_text(&mut control, "p2", "$");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
    until(
        "the window to show a tab",
        || !keyboard().0.is_empty(),
        || format!("the keyboard is at {:?}", keyboard()),
    );
    let (shown, _) = keyboard();

    // Before anything asks: the shell is told so, which its menu item greys itself out by.
    assert_eq!(read_asking(), muster::proto::Asking::default(), "something asks already");
    assert_eq!(told(), [false], "the shell was not told that nothing asks");

    // Whichever tab is not on screen asks, so going there has to change what is shown.
    let (hidden_tab, hidden_pane) = if shown == "t1" { ("t2", "p2") } else { ("t1", "p1") };
    let mut input = Input::connect(daemon.socket_path());
    let before_notification = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall clock")
            .as_millis(),
    )
    .expect("timestamp fits");
    let send = input_event::Send {
        text: r"printf '\033]9;look here\a'".to_string(),
        enter: true,
        ..Default::default()
    };
    input.send(hidden_pane, input_event::Input::Send(send));
    until(
        "the program's notification to ask for somebody",
        || asked().iter().any(|asked| asked.state == "notified" && asked.pane_id == hidden_pane),
        || format!("the window asked {:?}", asked()),
    );

    until(
        "the shell to be told something asks",
        || told() == [false, true],
        || format!("{:?}", told()),
    );
    let would = read_asking();
    assert_eq!(would.pane_id, hidden_pane, "would go to {would:?}");
    assert_eq!(keyboard().0, shown, "asking where it would go went there");
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        panic!("window")
    };
    let entry =
        window.asking.iter().find(|entry| entry.pane_id == hidden_pane).expect("notification asks");
    assert_eq!(entry.alert, "notified");
    assert!(
        entry.since_ms >= before_notification,
        "notification borrowed the pane's old state time"
    );
    let held = window
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .find(|pane| pane.pane_id == hidden_pane)
        .expect("the window's roster holds the pane that asks");
    assert!(!entry.callsign.is_empty(), "the entry names no callsign: {entry:?}");
    assert_eq!(
        (&entry.callsign, &entry.label),
        (&held.callsign, &held.label),
        "an entry names its pane as the roster does, so a window holding no roster for it can"
    );
    assert_rename_reaches_asking(hidden_pane);

    let went = focus_asking();
    assert_eq!(went, would, "went somewhere other than where it said it would");
    assert!(!went.daemon_id.is_empty(), "the answer names no daemon: {went:?}");
    assert_eq!(keyboard(), (hidden_tab.to_string(), hidden_pane.to_string()));

    // Somebody looks: the window has the OS's focus with the pane on screen.
    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    assert_eq!(read_asking(), muster::proto::Asking::default(), "something still asks");
    let nothing = focus_asking();
    assert_eq!(nothing, muster::proto::Asking::default(), "something still asks");
    assert_eq!(keyboard(), (hidden_tab.to_string(), hidden_pane.to_string()), "it moved");
    assert_eq!(told(), [false, true, false], "told only when the answer changed");
}

fn assert_rename_reaches_asking(hidden_pane: &str) {
    until(
        "the asking list to arrive",
        || {
            LISTS
                .lock()
                .expect("asking lists")
                .last()
                .is_some_and(|list| list.entries.iter().any(|entry| entry.pane_id == hidden_pane))
        },
        || "no asking list".to_string(),
    );
    assert_ok(&answer(request::Payload::RenamePane(RenamePane {
        pane_id: hidden_pane.to_string(),
        name: "renamed".to_string(),
        ..RenamePane::default()
    })));
    let label_in_list = || {
        LISTS
            .lock()
            .expect("asking lists")
            .last()
            .and_then(|list| list.entries.iter().find(|entry| entry.pane_id == hidden_pane))
            .map(|entry| entry.label.clone())
    };
    until(
        "the asking list to carry the new name",
        || label_in_list().as_deref() == Some("renamed"),
        || format!("the last list names the pane {:?}", label_in_list()),
    );
}

/// Muster quit and its agents went on, and one blocked meanwhile. The window that opens next
/// announces nothing about it, since it saw nothing change, and the pane is still the one asking:
/// the sidebar shows it blocked, so going to the pane that asked has to reach it.
///
/// Waits for the state the shell is told to paint, not for `ReadWindow`. That reads the daemon's
/// picture as the window last heard it, which is ahead of the asking list for as long as the
/// window takes to publish what it heard; the painted state is told after the list.
#[test]
fn a_pane_blocked_before_the_window_opened_is_reached() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    PAINTED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", in_new_tab("t2")));
    daemon.run_agent("p2");
    daemon.set_agent_state("p2", AgentState::Blocked);
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
    until(
        "the window to paint p2 blocked",
        || painted("p2").as_deref() == Some("blocked"),
        || {
            format!(
                "the window last painted p2 {:?}, and reads it as {:?}",
                painted("p2"),
                state_of("p2")
            )
        },
    );

    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        panic!("window")
    };
    let entry =
        window.asking.iter().find(|entry| entry.pane_id == "p2").expect("blocked pane asks");
    let pane = window.panes.iter().find(|pane| pane.pane_id == "p2").expect("blocked pane");
    assert_eq!(entry.alert, "blocked");
    assert_eq!(entry.since_ms, pane.since_ms);

    let went = focus_asking();
    assert_eq!(went.pane_id, "p2", "went to {went:?}");
    assert_eq!(keyboard().1, "p2");
}

/// A blocked pane's entry carries what it says it is blocked on, so a window that holds no roster
/// for the pane can draw it, and carries it only while the pane is blocked: that is the one card
/// which shows it, and a title changing under a pane that merely finished would resend the list.
#[test]
fn a_blocked_panes_entry_carries_its_subtitle_and_follows_the_title() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    LISTS.lock().expect("asking lists").clear();
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p2", in_new_tab("t2")));
    daemon.run_agent("p2");
    daemon.type_into("p2", "title Run the tests?");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }
    daemon.set_agent_state("p2", AgentState::Blocked);

    let entry = || {
        LISTS
            .lock()
            .expect("asking lists")
            .last()
            .and_then(|list| list.entries.iter().find(|entry| entry.pane_id == "p2").cloned())
    };
    let subtitle = || entry().map(|entry| entry.subtitle);
    until(
        "the blocked pane's entry to carry its title",
        || subtitle().as_deref() == Some("Run the tests?"),
        || format!("the last list holds {:?}", entry()),
    );
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        panic!("window")
    };
    let held = window
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .find(|pane| pane.pane_id == "p2")
        .expect("the window's roster holds the pane");
    assert_eq!(held.subtitle, "Run the tests?", "the roster and the entry disagree");

    daemon.type_into("p2", "title Which directory?");
    until(
        "a new title to reach the blocked pane's entry",
        || subtitle().as_deref() == Some("Which directory?"),
        || format!("the last list holds {:?}", entry()),
    );

    daemon.set_agent_state("p2", AgentState::Working);
    daemon.set_agent_state("p2", AgentState::Idle);
    until(
        "the pane that finished to ask as done",
        || entry().is_some_and(|entry| entry.alert == "done"),
        || format!("the last list holds {:?}", entry()),
    );
    assert_eq!(subtitle().as_deref(), Some(""), "a pane that is not blocked carries no subtitle");
}

/// What the window paints for a pane.
fn state_of(pane: &str) -> String {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return String::new();
    };
    window
        .panes
        .into_iter()
        .find(|agent| agent.pane_id == pane)
        .map(|agent| agent.state)
        .unwrap_or_default()
}

fn focus_asking() -> muster::proto::Asking {
    match answer(request::Payload::FocusAsking(FocusAsking {})).payload {
        Some(response::Payload::Asking(asking)) => asking,
        other => panic!("the core answered a FocusAsking with {other:?}"),
    }
}

fn read_asking() -> muster::proto::Asking {
    match answer(request::Payload::ReadAsking(ReadAsking {})).payload {
        Some(response::Payload::Asking(asking)) => asking,
        other => panic!("the core answered a ReadAsking with {other:?}"),
    }
}

/// The tab on screen and the pane with the keyboard.
fn keyboard() -> (String, String) {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return (String::new(), String::new());
    };
    let view = window.view.unwrap_or_default();
    (view.tab_id, view.keyboard_pane_id)
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

/// The state the shell was last told to paint for a pane.
fn painted(pane: &str) -> Option<String> {
    PAINTED
        .lock()
        .expect("a panicking test poisoned the log")
        .iter()
        .rev()
        .find(|(painted, _)| painted == pane)
        .map(|(_, state)| state.clone())
}

static PAINTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Every AskingChanged the shell was sent, in order.
static TOLD: Mutex<Vec<bool>> = Mutex::new(Vec::new());
static LISTS: Mutex<Vec<AskingListChanged>> = Mutex::new(Vec::new());

fn told() -> Vec<bool> {
    TOLD.lock().expect("a panicking test poisoned the log").clone()
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        Some(event::Payload::AskingChanged(changed)) => {
            TOLD.lock().expect("a panicking test poisoned the log").push(changed.asking);
        }
        Some(event::Payload::AskingListChanged(changed)) => {
            LISTS.lock().expect("asking lists").push(changed);
        }
        Some(event::Payload::PaneStateChanged(state)) => {
            PAINTED
                .lock()
                .expect("a panicking test poisoned the log")
                .push((state.pane_id, state.state));
        }
        _ => {}
    }
}
