//! A pane `muster pane new` just printed can be named by the very next command.
//!
//! `P=$(muster pane new); muster pane read --pane "$P"` is the shape every script driving agents
//! takes, and it was refused: a split answers with the pane's name as soon as the daemon has made
//! it, and the daemon's event describing the pane reaches the window a moment later. Every verb
//! that looks a name up in the window refused one it had not heard of yet (kan a_2P5nkSS8g).
//!
//! Each test splits over the command socket and asks straight away, the way a script does. Waiting
//! for the window to list the pane first would hide exactly the race under test.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use herdr_harness::{Daemon, until};
use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    ArrangePane, ClosePane, CreateTab, OpenWindow, ReadPane, ReadWindow, RenamePane, Request,
    Response, SendToPane, SplitPane, Startup, Window, request, response,
};
use prost::Message;
use serde_json::json;

#[test]
fn a_pane_is_read_the_moment_pane_new_prints_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::ReadPane(ReadPane { pane_id: made.clone(), ..ReadPane::default() }),
    );
    assert_did_it("pane read", &made, &answer);
}

#[test]
fn a_pane_is_sent_to_the_moment_pane_new_prints_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::SendToPane(SendToPane {
            pane_id: made.clone(),
            text: "hello".to_string(),
            ..SendToPane::default()
        }),
    );
    assert_did_it("pane send", &made, &answer);
}

#[test]
fn a_confirmed_send_reads_back_a_pane_pane_new_just_printed() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::SendToPane(SendToPane {
            pane_id: made.clone(),
            text: "hello".to_string(),
            confirm: true,
            ..SendToPane::default()
        }),
    );
    assert_did_it("pane send --confirm", &made, &answer);
}

#[test]
fn a_pane_is_renamed_the_moment_pane_new_prints_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::RenamePane(RenamePane {
            pane_id: made.clone(),
            name: "renamed at once".to_string(),
            ..RenamePane::default()
        }),
    );
    assert_did_it("pane rename", &made, &answer);
    // herdr announces a rename to nobody, so an answer of ok and a window that never shows the
    // name would be a rename that told the caller one thing and the person another.
    until(
        "the window to list the pane under the name it was given",
        || given_name(&read_window(&open.socket), &made) == Some("renamed at once"),
        || {
            format!(
                "the window lists {made} as {:?}",
                given_name(&read_window(&open.socket), &made)
            )
        },
    );
}

#[test]
fn a_pane_is_moved_the_moment_pane_new_prints_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::ArrangePane(ArrangePane {
            pane_id: made.clone(),
            onto_pane_id: open.pane.clone(),
            ..ArrangePane::default()
        }),
    );
    assert_did_it("pane move --onto", &made, &answer);
}

#[test]
fn a_pane_is_closed_the_moment_pane_new_prints_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = split(&open);

    let answer = dialed(
        &open.socket,
        request::Payload::ClosePane(ClosePane { pane_id: made.clone(), ..ClosePane::default() }),
    );
    assert_did_it("pane close", &made, &answer);
}

/// `tab new` prints a pane too, and reaches the window by a different request.
#[test]
fn a_pane_tab_new_prints_is_read_straight_away() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();
    let made = match dialed(
        &open.socket,
        request::Payload::CreateTab(CreateTab {
            pane_id: open.pane.clone(),
            cwd: "/tmp".to_string(),
            ..CreateTab::default()
        }),
    )
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("a new tab answered with {other:?} rather than the pane it made"),
    };

    let answer = dialed(
        &open.socket,
        request::Payload::ReadPane(ReadPane { pane_id: made.clone(), ..ReadPane::default() }),
    );
    assert_did_it("pane read after tab new", &made, &answer);
}

/// One window, showing one pane, driven over the socket a CLI dials.
struct Open {
    _daemon: Daemon,
    socket: PathBuf,
    pane: String,
}

fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "just-made", "focus": true }));

    let socket = daemon.root().join("command.sock");
    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    let mut pane = None;
    until(
        "the window to list the daemon's pane",
        || {
            pane = read_window(&socket).panes.first().map(|pane| pane.pane_id.clone());
            pane.is_some()
        },
        || "the window never listed a pane, so there was nothing to split".to_string(),
    );
    let pane = pane.expect("the wait above returns only once there is one");
    Open { _daemon: daemon, socket, pane }
}

/// Splits the window's pane over the socket, and hands back the name the split printed.
fn split(open: &Open) -> String {
    match dialed(
        &open.socket,
        request::Payload::SplitPane(SplitPane {
            pane_id: open.pane.clone(),
            side: "down".to_string(),
            ..SplitPane::default()
        }),
    )
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!("a split answered with {other:?} rather than the pane it made"),
    }
}

fn assert_did_it(verb: &str, made: &str, answer: &Response) {
    match &answer.payload {
        Some(response::Payload::Failure(failure)) => panic!(
            "`{verb}` refused {made}, a pane that had just been made: {}\n  Impact: a script \
             that names the pane it was just handed is told the pane is not there.",
            failure.reason
        ),
        Some(response::Payload::Unanswered(unanswered)) => {
            panic!("`{verb}` on {made} went unanswered: {}", unanswered.reason)
        }
        None => panic!("`{verb}` on {made} answered with nothing"),
        Some(_) => {}
    }
}

fn given_name<'a>(window: &'a Window, pane: &str) -> Option<&'a str> {
    window
        .roster
        .iter()
        .flat_map(|roster| roster.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .find(|listed| listed.pane_id == pane)
        .map(|listed| listed.given_name.as_str())
}

fn read_window(socket: &std::path::Path) -> Window {
    match dialed(socket, request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the endpoint answered a ReadWindow with {other:?}"),
    }
}

/// One request over one connection, and its one answer.
fn dialed(socket: &std::path::Path, payload: request::Payload) -> Response {
    let mut stream = UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("nothing is listening on {}: {error}", socket.display()));
    write_frame(&mut stream, &Request { payload: Some(payload) }.encode_to_vec())
        .expect("the endpoint takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the endpoint answers it");
    Response::decode(reply.as_slice()).expect("the answer is a response this build knows")
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request { payload: Some(payload) }.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
