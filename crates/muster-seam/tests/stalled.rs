//! A pane that was promised a bridge and never got one asks for another, against a real daemon.
//!
//! The gap `respawn.rs` cannot reach. There, every bridge that is replaced has first *ended*,
//! and ending is what advances the number the shell rebuilds a surface on. Here nothing ends:
//! the replacement the core decided on is never started, so no bridge exits, so nothing
//! advances the number, so no surface is built and no bridge runs - for the life of the app
//! process. Two agents were unreachable for ninety minutes that way, each holding an unsent
//! prompt (kan a_2KIPfvt7L).
//!
//! No bridge is started here and none is needed, on the same terms as `respawn.rs`: the view
//! is the whole of what the shell is told, and `bridge_restarts` moving is the only thing that
//! makes it build the surface a bridge is the command of.
//!
//! Its own binary because the deadline the watch runs on is read once per process, and the
//! other respawn tests must not run under a short one - a stall ask landing between their
//! bridge deaths would be a third thing moving the number they are counting.

use std::sync::Mutex;
use std::sync::{Once, atomic::AtomicUsize, atomic::Ordering};

use herdr_harness::{Daemon, until};
use muster::proto::{
    BridgeExited, Event, OpenWindow, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use prost::Message;

/// Short enough that the gate does not wait out the shipped five seconds, and long enough that
/// it is still a deadline rather than an immediate accusation - the daemon has to answer, the
/// view has to be published and a socket has to be bound before the clock even starts. The
/// same number `untypeable.rs` runs on, and for the same reasons.
const DEADLINE_MS: &str = "300";

#[test]
fn a_pane_whose_first_bridge_never_dials_is_asked_for_another() {
    // The launch half of the bug. Nothing has ended, so the replacement policy has never been
    // consulted about this pane - and until it is, the shell has no reason to build the
    // surface that would start a bridge.
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();
    let daemon = Daemon::start();
    let pane = open_a_window(&daemon);
    assert_eq!(restarts(&pane), Some(0), "a pane nobody has replaced is on none");

    until(
        "the core to ask for a bridge for a pane nothing has dialed",
        || restarts(&pane).is_some_and(|restarts| restarts > 0),
        || format!("the last view the core published: {:?}", latest_view()),
    );
}

#[test]
fn a_replacement_that_never_arrives_is_asked_for_again() {
    // The measured case. The core decided to replace - `bridge.replacing` is in the run log
    // with an attempt number - and no `bridge.start` followed it, so there was no bridge to
    // end and nothing that could ask a second time.
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();
    let daemon = Daemon::start();
    let pane = open_a_window(&daemon);

    // A bridge, then its death: the dial is what makes the pane typeable, so that the wait
    // starting again is a wait for the *replacement* rather than the first one still pending.
    let dialed = dial_a_bridge(&pane);
    report_exited(&pane, false);
    drop(dialed);
    until(
        "the core to count the replacement it decided on",
        || restarts(&pane) == Some(1),
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // Nothing dials it, which is the bug. A second number is the pane getting another chance
    // rather than staying dark until somebody quits the app.
    until(
        "the core to ask again for a replacement nothing started",
        || restarts(&pane).is_some_and(|restarts| restarts > 1),
        || format!("the last view the core published: {:?}", latest_view()),
    );
}

/// Sets the deadline this binary runs under, before any pane opens.
///
/// Once per process rather than once per test, because it is read once per process and a
/// second write would be the only thing in here touching the environment while a daemon the
/// last test started is still being torn down.
fn shorten_the_deadline() {
    static SET: Once = Once::new();
    SET.call_once(|| {
        // SAFETY: nothing else in this process reads the environment concurrently. This runs
        // before any daemon is started and before any pane opens, which is when the core
        // reads it.
        unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", DEADLINE_MS) };
    });
}

/// Connects to the pane's control socket the way a bridge starting would, and waits for the
/// core to notice.
fn dial_a_bridge(pane: &Pane) -> std::os::unix::net::UnixStream {
    let path = socket_of(pane).expect("the core publishes a control socket for every pane");
    let before = TYPEABLE.load(Ordering::Acquire);
    let stream = std::os::unix::net::UnixStream::connect(&path)
        .expect("the core is listening on the pane's socket");
    until(
        "the core to notice the bridge dial in",
        || TYPEABLE.load(Ordering::Acquire) > before,
        || format!("nothing was announced typeable after connecting to {path}"),
    );
    stream
}

/// Starts the core against this daemon, opens the window, and names the pane it came up on.
fn open_a_window(daemon: &Daemon) -> Pane {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
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

/// How many times the last published view says this pane has been given a bridge.
///
/// `None` is a view that does not name the pane at all, which is a different answer from zero.
fn restarts(pane: &Pane) -> Option<u32> {
    let view = latest_view()?;
    view.regions
        .iter()
        .filter(|region| region.daemon_id == pane.daemon)
        .filter_map(|region| region.root.as_ref())
        .find_map(|root| {
            let mut found = Vec::new();
            collect(root, &mut found);
            found.into_iter().find(|(id, _)| id == &pane.pane).map(|(_, restarts)| restarts)
        })
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
            Some(found.control_socket_path.clone())
        }
        Some(view_node::Node::Split(split)) => [split.first.as_deref(), split.second.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|child| find_socket(child, pane)),
        _ => None,
    }
}

fn first_pane() -> Option<Pane> {
    let view = latest_view()?;
    let region = view.regions.first()?;
    let mut found = Vec::new();
    collect(region.root.as_ref()?, &mut found);
    Some(Pane { daemon: region.daemon_id.clone(), pane: found.first()?.0.clone() })
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
