//! What a program in a pane asks of somebody, from the bytes it writes to what the window does
//! with them: a bell marks the pane, a notification asks in the program's own words, and
//! progress is shown with the pane's agent.

use std::sync::Mutex;

use muster::proto::{
    AttentionChanged, Event, OpenWindow, PaneStateChanged, ReadWindow, Request, Response, Startup,
    event, request, response,
};
use muster_daemon_proto::input_event;
use muster_harness::requests::{create, in_new_tab, make, snapshot, until_text};
use muster_harness::{Daemon, Input, until, until_some};
use prost::Message;

/// A window nobody is looking at hears a program in its pane ring, report progress and ask to
/// notify somebody, and does something different with each.
#[test]
fn a_bell_marks_its_pane_progress_shows_and_a_notification_asks_in_its_own_words() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    until_text(&mut daemon.connect(), "p1", "$");
    let socket = open_window(&daemon);

    let mut run = typing(&daemon);
    run(r"printf '\a\033]9;4;1;40\a'");
    let marked = until_some("the bell and the progress to reach the window", || {
        agent(&socket).filter(|agent| agent.rang && agent.progress.is_some())
    });
    let progress = marked.progress.expect("progress");
    assert_eq!((progress.state.as_str(), progress.percent), ("running", Some(40)));
    assert!(asked().is_empty(), "a bell asked for somebody: {:?}", asked());

    run(r"printf '\033]9;tests passed\a'");
    until(
        "the program's notification to ask for somebody",
        || asked().iter().any(|asked| asked.state == "notified"),
        || format!("the window asked {:?}", asked()),
    );
    let asked = asked().into_iter().find(|asked| asked.state == "notified").expect("asked");
    assert_eq!((asked.pane_id.as_str(), asked.note_body.as_str()), ("p1", "tests passed"));
}

/// A bell announces its pane when it marks it, and not again until somebody has looked: a shell
/// holding Tab rings for every keystroke, and each announcement is work on the shell's main
/// thread.
#[test]
fn a_bell_announces_its_pane_once_until_somebody_looks() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    ANNOUNCED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    until_text(&mut daemon.connect(), "p1", "$");
    let socket = open_window(&daemon);
    until_some("the window to list the pane", || agent(&socket));

    // A notification after the bells, which arrives after them and says they have all been
    // heard. Not progress: that announces the pane too, and can reach the mirror with the bells.
    typing(&daemon)(r"printf '\a\a\a\033]9;heard\a'");
    until(
        "the notification after the bells to ask for somebody",
        || asked().iter().any(|asked| asked.state == "notified"),
        || format!("the window asked {:?}", asked()),
    );
    let rang = ANNOUNCED
        .lock()
        .expect("a panicking test poisoned the log")
        .iter()
        .filter(|agent| agent.pane_id == "p1" && agent.rang)
        .count();
    assert_eq!(rang, 1, "three bells nobody heard announced the pane {rang} times");
}

/// An agent Muster recognizes notifies at the moments its state already asks for somebody, so
/// its own notification asks nobody. The same words from its shell once it has gone ask as any
/// program's do.
#[test]
fn an_agents_own_notification_asks_nobody_and_its_shells_programs_still_do() {
    let _turn = muster::testing::fresh_session();
    muster::ffi::muster_set_event_callback(Some(note));
    ASKED.lock().expect("a panicking test poisoned the log").clear();
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let socket = open_window(&daemon);
    until_some("the window to list the pane", || agent(&socket));

    let mut run = typing(&daemon);
    run("notify Claude is waiting for your input");
    run("quit");
    let mut control = daemon.connect();
    until_some("the daemon to see the agent leave", || {
        snapshot(&mut control)
            .panes
            .into_iter()
            .find(|pane| pane.pane == "p1" && pane.agent.is_none())
    });
    run(r"printf '\033]9;tests passed\a'");
    until(
        "the shell's notification to ask for somebody",
        || asked().iter().any(|asked| asked.state == "notified"),
        || format!("the window asked {:?}", asked()),
    );
    let bodies: Vec<String> = asked()
        .into_iter()
        .filter(|asked| asked.state == "notified")
        .map(|asked| asked.note_body)
        .collect();
    assert_eq!(
        bodies,
        ["tests passed"],
        "the agent's own notification asked for somebody, beside the state that already does"
    );
}

/// Opens a window onto the daemon's panes, nobody looking at it, and hands back its socket.
fn open_window(daemon: &Daemon) -> std::path::PathBuf {
    let socket = daemon.root().join("command.sock");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            command_socket_path: socket.to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        dispatch(payload);
    }
    socket
}

/// Types a line into p1 and presses Return.
fn typing(daemon: &Daemon) -> impl FnMut(&str) {
    let mut input = Input::connect(daemon.socket_path());
    move |line: &str| {
        let send = input_event::Send { text: line.to_string(), enter: true };
        input.send("p1", input_event::Input::Send(send));
    }
}

fn agent(socket: &std::path::Path) -> Option<PaneStateChanged> {
    let bytes = Request::new(request::Payload::ReadWindow(ReadWindow::default()));
    let window = match dialed(socket, &bytes).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the endpoint answered a ReadWindow with {other:?}"),
    };
    window.panes.into_iter().find(|agent| agent.pane_id == "p1")
}

fn dialed(socket: &std::path::Path, request: &Request) -> Response {
    use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the window listens");
    write_frame(&mut stream, &request.encode_to_vec()).expect("the request is written");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the window answers");
    Response::decode(reply.as_slice()).expect("an answer this build can decode")
}

fn dispatch(payload: request::Payload) {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response");
    if let Some(response::Payload::Failure(failure)) = reply.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

/// Every pane state the core told the shell, in order.
static ANNOUNCED: Mutex<Vec<PaneStateChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::AttentionChanged(asked)) => {
            ASKED.lock().expect("a panicking test poisoned the log").push(asked);
        }
        Some(event::Payload::PaneStateChanged(announced)) => {
            ANNOUNCED.lock().expect("a panicking test poisoned the log").push(announced);
        }
        _ => {}
    }
}
