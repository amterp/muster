//! Opening onto a daemon that holds nothing, which is what a fresh machine is.
//!
//! The path a bare launch takes on a machine where Muster just started its own herdr: nothing
//! to attach to, so a workspace is asked for, and the pane it makes arrives on the event
//! stream some milliseconds later. Every other test here starts from a daemon that already
//! holds panes, so this is the only one that exercises the order those events come in.
//!
//! What it asserts is an invariant rather than a sequence: a published view names a control
//! socket for every pane it shows. A shell must not spawn a bridge without one - it would
//! paint and then swallow every keystroke - so a pane published without one is a blank pane
//! that nothing republishes, which is exactly how this shipped once.
//!
//! Its own binary because the seam holds one session per process, and this needs a session
//! that has never seen a pane.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, Request, Response, Startup, ViewChanged, ViewNode, event, request, response,
    view_node,
};
use prost::Message;

#[test]
fn a_window_opened_on_an_empty_daemon_can_be_typed_into() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let config = daemon.muster_config();

    muster::ffi::muster_set_event_callback(Some(note_view));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));

    // A bare launch. Nothing names a pane, and on this daemon there is none to name.
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to show the workspace it asked for",
        || latest_view().is_some_and(|view| !panes(&view).is_empty()),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // The invariant, checked over whatever the view settled on rather than over one expected
    // shape. Without it the pane renders nothing at all: the shell defers the surface, logs
    // `pane.surface.deferred`, and waits for a republish that only a split ever caused.
    let view = latest_view().expect("just waited for it");

    // The keyboard has to be somewhere. A region with no pane is one where every keybinding
    // meaning "the focused pane" is refused - so ⌘T on a freshly opened window answered "no
    // pane has this window's keyboard" until somebody clicked, which looks like the window
    // ignoring the key.
    assert!(
        view.regions.iter().all(|region| !region.pane_id.is_empty()),
        "a region opened with the keyboard on no pane, so the first keybinding pressed is \
         refused: {view:?}"
    );
    let unreachable: Vec<String> = panes(&view)
        .into_iter()
        .filter(|(_, socket)| socket.is_empty())
        .map(|(pane, _)| pane)
        .collect();
    assert!(
        unreachable.is_empty(),
        "the view names {unreachable:?} with no control socket, so the shell cannot start a \
         bridge for them and they render as empty panes. The whole view: {view:?}"
    );
}

/// Every pane a view shows, with the socket a bridge for it would dial.
fn panes(view: &ViewChanged) -> Vec<(String, String)> {
    view.regions.iter().filter_map(|region| region.root.as_ref()).flat_map(leaves).collect()
}

fn leaves(node: &ViewNode) -> Vec<(String, String)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            vec![(pane.pane_id.clone(), pane.control_socket_path.clone())]
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note_view(bytes: *const u8, len: usize) {
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

/// That the core accepted a request, whichever shape its acceptance took.
///
/// `Made` is an acceptance too: a request that creates a pane answers with the pane rather than
/// with a bare Ok, because the name was minted inside the call and a caller cannot learn it any
/// other way. Only `Failure` is a refusal.
fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
