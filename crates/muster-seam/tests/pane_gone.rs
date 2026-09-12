//! A bridge told its terminal no longer exists is let go, against a real daemon.
//!
//! Closing a pane by hand put an error on its row five seconds later, telling the person to check
//! that the machine holding it was reachable - on a healthy daemon, about a pane that had closed
//! exactly as asked (kan a_2LMpvavhA). The bridge had heard herdr say the terminal was not found
//! and reported it as a lost connection, so the window started replacement bridges at a terminal
//! nothing can attach to and waited for one to dial.
//!
//! No bridge process runs here, on the same terms as `respawn.rs`: a bridge is a connection to
//! the pane's control socket and a line saying how it ended, so this dials the socket and says
//! what a real bridge says, spelled by the same function a bridge spells it with. The daemon
//! still lists the pane throughout, which is the moment the error came from: the terminal was
//! gone while the window was still drawing the pane.
//!
//! Its own binary because the deadline the watch runs on is read once per process.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, Once};
use std::time::Duration;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, ProblemsChanged, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use muster_herdr::bridge_report::{self, Exiting};
use prost::Message;

/// Short enough that the gate does not wait out the shipped five seconds. The same number
/// `untypeable.rs` and `stalled.rs` run on, and for the same reasons.
const DEADLINE_MS: &str = "300";

/// How long "and nothing was said about it" waits before it counts as true.
///
/// Three deadlines, as in `untypeable.rs`: there is no event for nothing arriving, so a negative
/// costs elapsed time. A replacement is published within milliseconds of the report, and the
/// error came one deadline after it.
const SETTLE: Duration = Duration::from_millis(900);

/// herdr's words to a client whose pane was closed under it, recorded in
/// `corpus/herdr-0.8.0/closing-reasons/closed.jsonl`.
const CLOSED_UNDER_IT: &str = "terminal attach ended: terminal term_65b513b45873d1 not found";

#[test]
fn a_bridge_whose_terminal_no_longer_exists_is_not_replaced_or_blamed_on_the_network() {
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();
    let daemon = Daemon::start();
    let log = daemon.root().join("run.jsonl");
    let pane = open_a_window(&daemon, &log);
    let socket = socket_of(&pane).expect("just waited for the pane's socket");

    let mut bridge = UnixStream::connect(&socket).expect("the core is listening on the socket");
    let said = Exiting {
        ending: bridge_report::ending(Some(CLOSED_UNDER_IT)),
        reason: Some(CLOSED_UNDER_IT.to_string()),
        rendered: true,
    };
    bridge.write_all(&said.wire_format()).expect("the core reads what a bridge says");
    drop(bridge);

    // Waited for rather than assumed, so a negative below is about what the window decided
    // and not about a report that had not arrived yet.
    until(
        "the core to hear how the bridge ended",
        || run_log(&log).lines().any(|line| line.contains("\"bridge.ended\"")),
        || format!("no bridge.ended in the run log:\n{}", run_log(&log)),
    );
    std::thread::sleep(SETTLE);

    assert_eq!(
        restarts(&pane),
        Some(0),
        "the window started another bridge for a pane whose daemon said its terminal no longer \
         exists, which nothing can ever attach to. Run log:\n{}",
        run_log(&log)
    );
    let about_it: Vec<_> =
        latest_problems().into_iter().filter(|problem| problem.key.contains(&pane)).collect();
    assert!(
        about_it.is_empty(),
        "the window raised a problem about a pane whose terminal is gone - the one this was \
         written for tells the person to check that a healthy machine is reachable: {about_it:?}"
    );
}

/// Starts the core against this daemon, opens the window, and names the pane it came up on once
/// that pane has a socket to dial.
fn open_a_window(daemon: &Daemon, log: &Path) -> String {
    watch_events();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        log_path: log.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a pane with a socket its bridge can dial",
        || first_pane().is_some_and(|pane| socket_of(&pane).is_some()),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    first_pane().expect("just waited for it")
}

fn run_log(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

fn shorten_the_deadline() {
    static SET: Once = Once::new();
    SET.call_once(|| {
        // SAFETY: nothing else in this process reads the environment concurrently. This runs
        // before any daemon is started and before any pane opens, which is when the core reads it.
        unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", DEADLINE_MS) };
    });
}

fn first_pane() -> Option<String> {
    let view = latest_view()?;
    let root = view.regions.first()?.root.as_ref()?;
    leaves(root).into_iter().next().map(|(pane, _, _)| pane)
}

fn socket_of(pane: &str) -> Option<String> {
    pane_in_view(pane).map(|(_, socket, _)| socket).filter(|socket| !socket.is_empty())
}

/// How many times the last published view says this pane's bridge has been replaced, or `None`
/// for a view that does not name the pane.
fn restarts(pane: &str) -> Option<u32> {
    pane_in_view(pane).map(|(_, _, restarts)| restarts)
}

fn pane_in_view(pane: &str) -> Option<(String, String, u32)> {
    latest_view()?
        .regions
        .iter()
        .filter_map(|region| region.root.as_ref())
        .flat_map(leaves)
        .find(|(id, _, _)| id == pane)
}

fn leaves(node: &ViewNode) -> Vec<(String, String, u32)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => {
            vec![(pane.pane_id.clone(), pane.control_socket_path.clone(), pane.bridge_restarts)]
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

/// Throws away what the last test published and listens again. The statics outlive a test where
/// the session does not.
fn watch_events() {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note));
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);
static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
        }
        Some(event::Payload::ProblemsChanged(problems)) => {
            *PROBLEMS.lock().expect("a panicking test poisoned the problems") = Some(problems);
        }
        _ => {}
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn latest_problems() -> Vec<muster::proto::Problem> {
    PROBLEMS
        .lock()
        .expect("a panicking test poisoned the problems")
        .clone()
        .map(|changed| changed.problems)
        .unwrap_or_default()
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
