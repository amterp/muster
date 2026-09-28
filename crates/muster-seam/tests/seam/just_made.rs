//! A pane `muster pane new` just printed can be named by the very next command.
//!
//! `P=$(muster pane new); muster pane read --pane "$P"` is the shape every script driving agents
//! takes, and it was refused once: a split answered with the pane's name as soon as the daemon had
//! made it, and the daemon's event describing the pane reached the window a moment later. Every
//! verb that looks a name up in the window refused one it had not heard of yet (kan a_2P5nkSS8g).
//! A submit now returns with its effect already in the window's mirror, which closes that gap by
//! construction; these tests are what keeps it closed.
//!
//! Each test splits over the command socket and asks straight away, the way a script does. Waiting
//! for the window to list the pane first would hide exactly the race under test.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    ArrangePane, ClosePane, CreateTab, OpenWindow, ReadPane, ReadWindow, RenamePane, Request,
    Response, SendToPane, SplitPane, Startup, Window, request, response,
};
use muster_harness::{Daemon, until_some};
use prost::Message;

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
    // Asserted rather than waited for: the rename's answer arrives after the daemon's event
    // saying so, and an answer of ok with a window that does not yet show the name would be a
    // rename that told the caller one thing and the person another.
    let window = read_window(&open.socket);
    assert_eq!(
        given_name(&window, &made),
        Some("renamed at once"),
        "the rename was answered, and the window does not list {made} under the new name"
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

/// The pane is the one the window asks its empty daemon for while it opens.
fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start_built();

    let socket = daemon.root().join("command.sock");
    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    // Waited for, because opening asks for the first tab only once the daemon has sent its
    // first snapshot, and on a loaded machine that arrives after opening has returned. The race
    // these tests are about is the split's, which each test asks about straight away.
    let pane = until_some("the window to open onto the first tab it asked for", || {
        read_window(&socket).panes.first().map(|pane| pane.pane_id.clone())
    });
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
