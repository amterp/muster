//! A bridge that dials in for a pane already hidden is told so, against a real daemon.
//!
//! A hidden pane's bridge lets go of its herdr client, which is what stops the daemon rendering
//! it. The core tells every bridge when the set of panes on screen changes - but a pane hidden
//! before its bridge has dialed was told nothing, since there was no connection to write to,
//! and it streamed until the set next changed.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::Duration;

use herdr_harness::{Daemon, until};
use muster::proto::{
    CreateTab, Event, OpenWindow, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use prost::Message;

#[test]
fn a_pane_hidden_before_its_bridge_dials_is_told_when_it_does() {
    let _turn = muster::testing::fresh_session();
    // No bridge runs here, so a watchdog would otherwise start republishing on its own clock.
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a pane with a socket",
        || first_pane().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let (pane, socket) = first_pane().expect("just waited for it");

    // A second tab takes the screen before anything has dialed the first pane's socket.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the first pane to leave the screen",
        || latest_view().is_some_and(|view| !format!("{view:?}").contains(&pane)),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    let stream = UnixStream::connect(&socket).expect("the core is still listening for its bridge");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("a read timeout");
    let mut line = String::new();
    let _ = BufReader::new(&stream).read_line(&mut line);
    assert!(
        line.contains("\"bridge.showing\"") && line.contains("\"on_screen\":false"),
        "a bridge dialing in for a hidden pane was not told it is hidden, so it streams a pane \
         nobody can see: {line:?}"
    );
}

/// The pane the window opened onto, and the socket its bridge would dial.
fn first_pane() -> Option<(String, String)> {
    let view = latest_view()?;
    let root = view.regions.first()?.root.clone()?;
    find_pane(&root)
}

fn find_pane(node: &ViewNode) -> Option<(String, String)> {
    match node.node.as_ref()? {
        view_node::Node::Pane(pane) if !pane.control_socket_path.is_empty() => {
            Some((pane.pane_id.clone(), pane.control_socket_path.clone()))
        }
        view_node::Node::Split(split) => [split.first.as_deref(), split.second.as_deref()]
            .into_iter()
            .flatten()
            .find_map(find_pane),
        view_node::Node::Pane(_) => None,
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ViewChanged(view)) = event.payload {
        *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
