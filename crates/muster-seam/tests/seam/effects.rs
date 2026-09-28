//! What a program in a pane asks of somebody, from the bytes it writes to what the window does
//! with them: a bell marks the pane, a notification asks in the program's own words, and
//! progress is shown with the pane's agent.

use std::sync::Mutex;

use muster::proto::{
    AttentionChanged, Event, OpenWindow, PaneStateChanged, ReadWindow, Request, Response, Startup,
    event, request, response,
};
use muster_daemon_proto::input_event;
use muster_harness::requests::{create, in_new_tab, make, until_text};
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
    let socket = daemon.root().join("command.sock");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            command_socket_path: socket.to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow {}),
    ] {
        dispatch(payload);
    }
    until_some("the window to list the pane", || agent(&socket));

    let mut input = Input::connect(daemon.socket_path());
    let mut run = |command: &str| {
        let send = input_event::Send { text: command.to_string(), enter: true };
        input.send("p1", input_event::Input::Send(send));
    };
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

fn agent(socket: &std::path::Path) -> Option<PaneStateChanged> {
    let bytes = Request { payload: Some(request::Payload::ReadWindow(ReadWindow {})) };
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
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response");
    if let Some(response::Payload::Failure(failure)) = reply.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

fn asked() -> Vec<AttentionChanged> {
    ASKED.lock().expect("a panicking test poisoned the log").clone()
}

static ASKED: Mutex<Vec<AttentionChanged>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::AttentionChanged(asked)) = event.payload {
        ASKED.lock().expect("a panicking test poisoned the log").push(asked);
    }
}
