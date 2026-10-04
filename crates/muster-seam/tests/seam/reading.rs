//! A pane's text, read back through the window's command socket the way `muster pane read` asks.

use muster::proto::{
    OpenWindow, ReadPane, ReadWindow, Request, Response, Startup, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::input_event;
use muster_harness::requests::{create, in_new_tab, make, screen_text, until_text};
use muster_harness::{Daemon, Input, until_some};
use prost::Message;

/// A read of a pane's whole history reaches its caller however large it is. The daemon answers
/// in pages of up to 4 MiB of text, and a history longer than the window's answer could carry
/// left `muster pane read` with nothing at all.
#[test]
fn a_whole_history_larger_than_a_mebibyte_reaches_its_caller() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    until_text(&mut daemon.connect(), "p1", "$");
    let socket = daemon.root().join("command.sock");
    let config = daemon.muster_config_with("scrollback_bytes = 1073741824");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: config.to_string_lossy().into_owned(),
            command_socket_path: socket.to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        dispatch(payload);
    }
    until_some("the window to list the pane", || {
        match dialed(&socket, request::Payload::ReadWindow(ReadWindow::default())).payload {
            Some(response::Payload::Window(window)) => {
                window.panes.iter().any(|pane| pane.pane_id == "p1").then_some(())
            }
            _ => None,
        }
    });

    // About 2.1 MB of text, in rows too short to wrap.
    let mut input = Input::connect(daemon.socket_path());
    let print = "awk 'BEGIN { for (i = 0; i < 30000; i++) printf \"%070d\\n\", i; \
                 print \"LONG-END\" }'";
    let send = input_event::Send { text: print.to_string(), enter: true };
    input.send("p1", input_event::Input::Send(send));
    until_text(&mut daemon.connect(), "p1", "LONG-END\n");

    let read = dialed(
        &socket,
        request::Payload::ReadPane(ReadPane { pane_id: "p1".to_string(), ..ReadPane::default() }),
    );
    let Some(response::Payload::PaneText(text)) = read.payload else {
        panic!("a whole read answered with {:?}", read.payload);
    };
    assert!(text.text.len() > 1 << 20, "{} bytes of a 2 MB history", text.text.len());
    assert!(text.text.contains("LONG-END"), "the read ends at the newest row");
}

/// A read of the turn reaches the daemon through the window, and comes back saying it is one:
/// what the agent printed since it went to work, and not what it printed before.
#[test]
fn a_read_of_the_last_turn_is_what_the_agent_printed_since_it_went_to_work() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
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
    let say = |control: &mut muster_harness::Control, text: &str| {
        let send = input_event::Send { text: format!("say {text}"), enter: true };
        Input::connect(daemon.socket_path()).send("p1", input_event::Input::Send(send));
        until_some(&format!("the agent to print {text:?}"), || {
            let screen = screen_text(control, "p1");
            (screen.contains(text) && !screen.contains("say ")).then_some(())
        });
    };
    say(&mut control, "before the turn");
    daemon.unblock_agent_unasked();
    daemon.until_agent("p1", daemon_proto::AgentState::Working);
    say(&mut control, "the report");

    let read = until_some("the window to read the turn", || {
        let asked = ReadPane { pane_id: "p1".to_string(), turn: true, ..ReadPane::default() };
        match dialed(&socket, request::Payload::ReadPane(asked)).payload {
            Some(response::Payload::PaneText(text)) => Some(text),
            _ => None,
        }
    });
    assert!(read.turn, "the answer does not say it is the turn: {read:?}");
    assert!(read.text.starts_with("the report"), "{read:?}");
    assert!(!read.text.contains("before the turn"), "{read:?}");
}

fn dialed(socket: &std::path::Path, payload: request::Payload) -> Response {
    use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
    let request = Request::new(payload);
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the window listens");
    write_frame(&mut stream, &request.encode_to_vec()).expect("the request is written");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE)
        .unwrap_or_else(|error| panic!("the window's answer could not be read: {error}"));
    Response::decode(reply.as_slice()).expect("an answer this build can decode")
}

fn dispatch(payload: request::Payload) {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response");
    if let Some(response::Payload::Failure(failure)) = reply.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
