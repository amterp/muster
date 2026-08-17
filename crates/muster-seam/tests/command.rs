//! Whether something outside this process can drive the window, and see what it did.
//!
//! The gesture the board has been missing (kan `a_298rp2Pss`): not "the dispatcher answers a
//! `ReadWindow`", which a call to [`muster::dispatch`] would prove, but a real socket, dialed
//! by a caller that shares nothing with the app but the schema. Everything between - the
//! framing, the accept loop, the thread per connection, the endpoint being bound at all - has
//! no other test, and each of them fails as "the CLI does nothing".
//!
//! A real daemon behind it, because the answer is about panes and a stand-in would be Muster's
//! own guess at what a daemon holds.
//!
//! One test in this binary, on purpose: the seam holds one session per process.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use herdr_harness::Daemon;
use muster::command::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, Startup, Window, request, response,
};
use prost::Message;
use serde_json::json;

#[test]
fn a_caller_outside_this_process_can_ask_what_the_window_is_showing() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "driven", "focus": true }));

    // Inside the daemon's own scratch directory, so the run leaves nothing behind and two runs
    // of this test in parallel cannot collide on one path.
    let socket = daemon.root().join("command.sock");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    // What a caller has to be able to learn, in the order it stops being useful without it:
    // which panes exist, whether the picture can be trusted, and what is drawn where.
    until(
        "the window to describe the pane the daemon holds",
        &socket,
        |window| named_panes(window).len() == 1,
        "no pane reached the answer, so a script could see the window and nothing in it",
    );

    let window = read_window(&socket);
    let panes = named_panes(&window);
    assert_eq!(
        window.daemons.iter().map(|d| d.state.as_str()).collect::<Vec<_>>(),
        vec!["connected"],
        "the answer has to say how much of the daemon's truth Muster has. Without it a caller \
         acts on an hour-old mirror exactly as it would on a live one, and nothing in the \
         answer distinguishes them: {window:?}"
    );
    assert_eq!(
        window.view.as_ref().and_then(|view| view.regions.first()).map(|r| r.pane_id.clone()),
        Some(panes[0].clone()),
        "the region the window draws should name the pane the keyboard is on, by the name \
         Muster calls it - which is the name a caller would send back: {window:?}"
    );
    assert!(
        panes[0].starts_with('p'),
        "a caller is answered with Muster's own name for a pane, never the daemon's - a herdr \
         id is not unique across machines and is not addressable. Got {:?}",
        panes[0]
    );
    assert_eq!(
        window.panes.iter().map(|pane| pane.pane_id.clone()).collect::<Vec<_>>(),
        panes,
        "every pane's agent state travels with the window. A caller that has to ask again per \
         pane cannot see them at one moment: {window:?}"
    );

    // Two callers at once, because a thread per connection is the reason a `pane new` waiting
    // on a shell prompt does not hold up somebody asking what the window looks like. Dialed
    // before either is answered, so this fails on an accept loop that serves one at a time.
    let (mut first, mut second) = (dial(&socket), dial(&socket));
    let asking =
        Request { payload: Some(request::Payload::ReadWindow(ReadWindow {})) }.encode_to_vec();
    write_frame(&mut first, &asking).expect("the endpoint takes a request");
    write_frame(&mut second, &asking).expect("the endpoint takes a second request");
    for (which, stream) in [("first", &mut first), ("second", &mut second)] {
        let reply = read_frame(stream, LARGEST_MESSAGE)
            .unwrap_or_else(|detail| panic!("the {which} caller went unanswered: {detail}"));
        assert!(
            matches!(
                Response::decode(reply.as_slice()).map(|r| r.payload),
                Ok(Some(response::Payload::Window(_)))
            ),
            "the {which} caller of two dialing at once got no window"
        );
    }

    // A caller claiming a message too big to be one is refused without the bytes being read, so
    // that anything able to dial a unix socket cannot make the app reserve a gigabyte.
    let mut absurd = dial(&socket);
    absurd.write_all(&(LARGEST_MESSAGE + 1).to_be_bytes()).expect("the endpoint takes a length");
    let mut answered = Vec::new();
    absurd.read_to_end(&mut answered).expect("a refused caller is hung up on, not left waiting");
    assert!(
        answered.is_empty(),
        "an over-long request should be hung up on rather than answered: {answered:?}"
    );

    // And the window is still answering, which is the half that matters: a refusal that took
    // the accept loop with it would leave every later caller talking to nothing.
    assert_eq!(
        named_panes(&read_window(&socket)),
        panes,
        "the endpoint stopped answering after refusing one caller, so one bad client is enough \
         to make this window undriveable for the rest of the run"
    );
}

/// Every pane in the answer, by the name Muster calls it.
fn named_panes(window: &Window) -> Vec<String> {
    window
        .roster
        .iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| pane.pane_id.clone())
        .collect()
}

fn read_window(socket: &std::path::Path) -> Window {
    let response = dialed(socket, request::Payload::ReadWindow(ReadWindow {}));
    match response.payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the endpoint answered a ReadWindow with {other:?}"),
    }
}

/// One request over one connection, which is the whole of the protocol.
fn dialed(socket: &std::path::Path, payload: request::Payload) -> Response {
    let mut stream = dial(socket);
    let asking = Request { payload: Some(payload) }.encode_to_vec();
    write_frame(&mut stream, &asking).expect("the endpoint takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the endpoint answers it");
    Response::decode(reply.as_slice()).expect("the answer is a response this build knows")
}

fn dial(socket: &std::path::Path) -> UnixStream {
    UnixStream::connect(socket).unwrap_or_else(|error| {
        panic!(
            "nothing is listening on {}: {error}. Startup binds the endpoint, so this is the \
             window never having opened one rather than a request going wrong.",
            socket.display()
        )
    })
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}

/// Polls the endpoint rather than sleeping on it, and says what it was waiting for.
fn until(
    what: &str,
    socket: &std::path::Path,
    mut ready: impl FnMut(&Window) -> bool,
    impact: &str,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        let window = read_window(socket);
        if ready(&window) {
            return;
        }
        last = Some(window);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out after 15s waiting for {what}.\n  Impact: {impact}\n  Last answer: {last:?}");
}
