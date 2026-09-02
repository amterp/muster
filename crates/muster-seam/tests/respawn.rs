//! A pane whose bridge died, and what the window does about it, against a real daemon.
//!
//! `composition/respawn.json` pins the rule - how many replacements a pane gets, and how long
//! one must last to count as having worked. What needs a daemon is the wiring around it: that
//! a bridge ending reaches the core at all, that the core asks the daemon whether the pane is
//! still there, and that the answer comes back out as a view the shell can act on.
//!
//! No bridge is started here and none is needed. The view is the whole of what the shell is
//! told - `bridge_restarts` on a pane is what makes it build a new surface, and building one is
//! the only way a bridge is ever started - so a view carrying the number is the seam under test.

use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_harness::{Daemon, until};
use muster::proto::{
    BridgeExited, Event, OpenWindow, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use prost::Message;

#[test]
fn a_bridge_that_died_on_a_pane_the_daemon_still_holds_is_replaced() {
    // The bug: unplugging ethernet and picking up wifi kills the ssh under every devenv pane,
    // and nothing started another bridge. The panes stayed on screen showing a dead terminal
    // until Muster was relaunched, and every network change left another one.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let pane = open_a_window(&daemon);
    assert_eq!(restarts(&pane), Some(0), "a pane nobody has replaced is on none");

    report_exited(&pane, false);

    // One, and published: the count is what the shell compares against what it has, so a
    // decision the core kept to itself would leave the pane exactly as dead as before.
    until(
        "the core to publish the pane on its first replacement",
        || restarts(&pane) == Some(1),
        || format!("the last view the core published: {:?}", latest_view()),
    );
}

#[test]
fn a_surface_muster_tore_down_gets_no_replacement() {
    // The other reason a surface ends, and the one that must not start anything: Muster took
    // the pane off screen, or is rebuilding its surface for a reason of its own. A replacement
    // here would race the bridge that is already on its way.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let pane = open_a_window(&daemon);

    report_exited(&pane, true);

    assert_eq!(
        restarts(&pane),
        Some(0),
        "Muster replaced a bridge it had ended itself, which is a second one racing the first"
    );
}

#[test]
fn a_bridge_that_keeps_dying_is_given_up_on() {
    // The negative case the card asks for, and the reason the rule is not "always replace": a
    // bridge that cannot attach ends in a fraction of a second, so without a limit this is a
    // process every few hundred milliseconds for as long as the window is open.
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let pane = open_a_window(&daemon);

    for _ in 0..5 {
        // A bridge, then its death, five times over. The dial is not decoration: the core
        // takes one death per bridge, because two things watch a bridge die and the second
        // arrival is not a second death - so five reports with nothing in between are one
        // bridge reported five times, which is not what this is about.
        let dialed = dial_a_bridge(&pane);
        report_exited(&pane, false);
        drop(dialed);
    }

    // Three, not five. Every exit here lands within milliseconds of the last, so none of the
    // bridges counted as having worked.
    assert_eq!(restarts(&pane), Some(3));
}

/// Connects to the pane's control socket the way a bridge starting would, and waits for the
/// core to notice.
///
/// The connection is what the app treats as a bridge existing, so this is how a test that
/// starts no processes says one arrived.
fn dial_a_bridge(pane: &Pane) -> UnixStream {
    let path = socket_of(pane).expect("the core publishes a control socket for every pane");
    let before = typeable_count();
    let stream = UnixStream::connect(&path).expect("the core is listening on the pane's socket");
    until("the core to notice the bridge dial in", || typeable_count() > before, || {
        format!("nothing was announced typeable after connecting to {path}")
    });
    stream
}

/// Starts the core against this daemon, opens the window, and names the pane it came up on.
fn open_a_window(daemon: &Daemon) -> Pane {
    forget_the_view();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a workspace",
        || first_pane().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    first_pane().expect("just waited for it")
}

/// A pane, named the way anything crossing the seam names one.
#[derive(Debug, Clone)]
struct Pane {
    daemon: String,
    pane: String,
}

fn report_exited(pane: &Pane, process_alive: bool) {
    assert_ok(&answer(request::Payload::BridgeExited(BridgeExited {
        daemon_id: pane.daemon.clone(),
        pane_id: pane.pane.clone(),
        process_alive,
    })));
}

/// Where the last published view puts this pane's control socket, which is what a bridge dials.
fn socket_of(pane: &Pane) -> Option<String> {
    let view = latest_view()?;
    view.regions
        .iter()
        .filter(|region| region.daemon_id == pane.daemon)
        .filter_map(|region| region.root.as_ref())
        .find_map(|root| find_socket(root, &pane.pane))
}

fn find_socket(node: &ViewNode, pane: &str) -> Option<String> {
    match node.node.as_ref() {
        Some(view_node::Node::Pane(found)) if found.pane_id == pane => {
            Some(found.control_socket_path.clone()).filter(|path| !path.is_empty())
        }
        Some(view_node::Node::Split(split)) => [split.first.as_deref(), split.second.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|child| find_socket(child, pane)),
        _ => None,
    }
}

/// How many times the last published view says this pane's bridge has been replaced.
///
/// `None` is a view that does not name the pane at all, which is a different answer from zero
/// and the one a test that closed a pane would be looking at.
fn restarts(pane: &Pane) -> Option<u32> {
    let view = latest_view()?;
    view.regions
        .iter()
        .filter(|region| region.daemon_id == pane.daemon)
        .filter_map(|region| region.root.as_ref())
        .find_map(|root| find(root, &pane.pane))
}

fn first_pane() -> Option<Pane> {
    let view = latest_view()?;
    let region = view.regions.first()?;
    let mut found = Vec::new();
    collect(region.root.as_ref()?, &mut found);
    Some(Pane { daemon: region.daemon_id.clone(), pane: found.first()?.0.clone() })
}

fn find(node: &ViewNode, pane: &str) -> Option<u32> {
    let mut found = Vec::new();
    collect(node, &mut found);
    found.into_iter().find(|(id, _)| id == pane).map(|(_, restarts)| restarts)
}

fn collect(node: &ViewNode, into: &mut Vec<(String, u32)>) {
    match node.node.as_ref() {
        Some(view_node::Node::Pane(pane)) => {
            into.push((pane.pane_id.clone(), pane.bridge_restarts));
        }
        Some(view_node::Node::Split(split)) => {
            for child in [split.first.as_deref(), split.second.as_deref()].into_iter().flatten() {
                collect(child, into);
            }
        }
        None => {}
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

/// How many panes the core has announced typeable, which is how a test knows a dial landed.
static TYPEABLE: AtomicUsize = AtomicUsize::new(0);

fn typeable_count() -> usize {
    TYPEABLE.load(Ordering::Acquire)
}

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        Some(event::Payload::PaneTypeable(_)) => {
            TYPEABLE.fetch_add(1, Ordering::Release);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

/// Throws away what the last test published, so a wait here cannot be answered by the window
/// before it - the statics outlive a test where the session does not.
fn forget_the_view() {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
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
