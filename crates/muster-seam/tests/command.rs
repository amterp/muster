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
use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, SendToPane, SplitPane, Startup, Window, request,
    response,
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
    let log = daemon.root().join("run.jsonl");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        log_path: log.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    the_run_log_says_where_this_window_is_listening(&log, &socket);

    // What a caller has to be able to learn, in the order it stops being useful without it:
    // which panes exist, whether the picture can be trusted, and what is drawn where.
    until(
        "the window to describe the pane the daemon holds",
        &socket,
        |window| named_panes(window).len() == 1,
        "no pane reached the answer, so a script could see the window and nothing in it",
    );

    let panes = the_answer_carries_what_a_caller_needs(&read_window(&socket));

    two_callers_are_both_answered(&socket);
    an_over_long_claim_is_hung_up_on(&socket);
    // The window is still answering, which is the half of that which matters: a refusal that took
    // the accept loop with it would leave every later caller talking to nothing.
    assert_eq!(
        named_panes(&read_window(&socket)),
        panes,
        "the endpoint stopped answering after refusing one caller, so one bad client is enough \
         to make this window undriveable for the rest of the run"
    );

    // The gesture itself: make a pane below this one, running something, called something, and
    // without moving the keyboard. This is the whole shape a script and an agent send.
    let ran = daemon.root().join("integrator-ran.txt");
    let made = match dialed(
        &socket,
        request::Payload::SplitPane(SplitPane {
            pane_id: panes[0].clone(),
            side: "down".to_string(),
            run: format!("printf 'ran' > {}", ran.display()),
            name: "🤖 A".to_string(),
            // Left false, which is what makes this a script rather than a keystroke.
            ..SplitPane::default()
        }),
    )
    .payload
    {
        Some(response::Payload::Made(made)) => made.pane_id,
        other => panic!(
            "a split has to answer with the pane it made - a caller that cannot learn the name \
             cannot address it, and the name was minted inside that call. Got {other:?}"
        ),
    };
    assert!(
        !made.is_empty() && made != panes[0],
        "the split answered with {made:?}, which is not a new pane"
    );

    // The keyboard stayed put. What pressing a key means is "I made this and I am looking at
    // it"; what a script means is "make it and leave my cursor alone", and an agent opening
    // three panes would otherwise drag somebody's cursor through all three.
    let window = read_window(&socket);
    assert_eq!(
        window.view.as_ref().and_then(|view| view.regions.first()).map(|r| r.pane_id.clone()),
        Some(panes[0].clone()),
        "a split that did not ask for focus took it anyway: {window:?}"
    );

    // Named, and named in the window rather than only on the daemon - herdr announces a rename
    // to nobody, so this is the assertion that the reply was read.
    until(
        "the window to list the pane under the name the split asked for",
        &socket,
        |window| given_names(window).contains(&"🤖 A".to_string()),
        "the pane was made and the window shows it unnamed, so somebody running several agents \
         cannot tell it from the others",
    );

    // And what was asked to run, ran. Read off the filesystem rather than the pane's screen: a
    // grid wraps at its width and carries the shell's own echo of the command, so reading one
    // cannot tell "it ran" from "it is sitting at the prompt".
    until_file(&ran, "the command the split asked for to have run");

    // Text to a pane by name, which is the other half of an agent instructing another. Sent
    // through the endpoint to the pane the split made, and read back off that pane's own screen -
    // where it has to appear, because appearing is the whole point.
    let echoed = daemon.root().join("integrator-echoed.txt");
    assert_ok(&dialed(
        &socket,
        request::Payload::SendToPane(SendToPane {
            pane_id: made.clone(),
            text: format!("printf 'told' > {}", echoed.display()),
            enter: true,
            ..SendToPane::default()
        }),
    ));
    until_file(&echoed, "text sent to the pane by name to have been run there");
}

/// What a caller has to be able to learn, in the order it stops being useful without it: which
/// panes exist, whether the picture can be trusted, and what is drawn where.
///
/// Returns the panes, by the name Muster calls them, which is what everything after this addresses.
fn the_answer_carries_what_a_caller_needs(window: &Window) -> Vec<String> {
    let panes = named_panes(window);
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
    panes
}

/// The one line anybody reads when the CLI cannot find a window.
///
/// It went missing for a while, because the endpoint was bound before logging was installed - so
/// every launch answered "where is this window listening" to nowhere. Found by running the app,
/// and nothing else would have noticed: the endpoint itself worked.
fn the_run_log_says_where_this_window_is_listening(
    log: &std::path::Path,
    socket: &std::path::Path,
) {
    let recorded = std::fs::read_to_string(log).expect("the run log was opened");
    assert!(
        recorded.contains("command.listening")
            && recorded.contains(&socket.to_string_lossy().into_owned()),
        "the run log does not say where this window is listening, so a caller that cannot find \
         one has nothing to read. Log:\n{recorded}"
    );
}

/// Two callers dialing before either is answered both get an answer.
///
/// A thread per connection is why a `pane new` waiting on a shell prompt does not hold up somebody
/// asking what the window looks like. Both connections are opened and both requests written before
/// either reply is read, so an accept loop that served one at a time would deadlock here rather
/// than merely being slow.
fn two_callers_are_both_answered(socket: &std::path::Path) {
    let (mut first, mut second) = (dial(socket), dial(socket));
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
}

/// A caller claiming a message too big to be one gets nothing, and its bytes are never read.
///
/// Anything that can dial a unix socket can send a length. Without a ceiling, a port scanner or a
/// client built against another schema could make the window reserve a gigabyte by saying it was
/// about to send one.
fn an_over_long_claim_is_hung_up_on(socket: &std::path::Path) {
    let mut absurd = dial(socket);
    absurd.write_all(&(LARGEST_MESSAGE + 1).to_be_bytes()).expect("the endpoint takes a length");
    let mut answered = Vec::new();
    absurd.read_to_end(&mut answered).expect("a refused caller is hung up on, not left waiting");
    assert!(
        answered.is_empty(),
        "an over-long request should be hung up on rather than answered: {answered:?}"
    );
}

/// Every name a person gave a pane, as the window lists them.
fn given_names(window: &Window) -> Vec<String> {
    window
        .roster
        .iter()
        .flat_map(|roster| roster.daemons.iter())
        .flat_map(|daemon| daemon.tabs.iter())
        .flat_map(|tab| tab.panes.iter())
        .map(|pane| pane.given_name.clone())
        .collect()
}

/// Waits for a file a shell in a pane was asked to write.
///
/// The oracle for "did this actually run": the split's answer comes back before the shell has
/// finished being typed into, so how long this takes is the machine's business.
fn until_file(path: &std::path::Path, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|text| !text.is_empty()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "waited 20s for {what}, and {} was never written.\n  Impact: the pane exists and is \
         sitting at its own prompt, which looks exactly like a command that ran and printed \
         nothing.",
        path.display()
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
