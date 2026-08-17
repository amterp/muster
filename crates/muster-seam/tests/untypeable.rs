//! A pane that renders and never becomes typeable says so, without anybody typing into it.
//!
//! Three bugs in this repo ended the same way: a pane paints, swallows every keystroke, and
//! nothing mentions it. The bridge failed to dial, the socket path had moved, the channel
//! could not be opened - one symptom, and the first person to know was whoever typed.
//!
//! This is that symptom on purpose. Every test here attaches a real daemon and starts no
//! bridge, because the shell is what starts one and there is no shell - so the pane the daemon
//! makes is genuinely deaf, and what is being proved is that the core notices rather than that
//! it can be made to.
//!
//! Its own binary because the seam holds one session per process, and this needs a process
//! whose deadline was set before any pane opened.

use std::sync::Mutex;

use herdr_harness::Daemon;
use muster::proto::{
    Event, OpenWindow, ProblemsChanged, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use prost::Message;

/// Short enough that the gate does not wait out the shipped five seconds, and long enough that
/// it is still a deadline rather than an immediate accusation - the daemon has to answer, the
/// view has to be published, and a socket has to be bound before the clock even starts.
const DEADLINE_MS: &str = "300";

#[test]
fn a_pane_whose_bridge_never_dials_is_reported() {
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", DEADLINE_MS) };

    let daemon = Daemon::start();
    let config = daemon.muster_config();

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to show the pane it asked for",
        || !panes().is_empty(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let pane = panes().pop().expect("just waited for one");

    until(
        "the core to report a pane that never became typeable",
        || !latest_problems().is_empty(),
        || {
            format!(
                "nothing was reported {DEADLINE_MS}ms after a socket was bound for {pane} and \
                 no bridge dialed it. That is the whole of this feature: without it the pane \
                 renders, swallows every keystroke, and the window says nothing"
            )
        },
    );

    let problems = latest_problems();
    assert_eq!(
        problems.len(),
        1,
        "one deaf pane should be one problem, and this window has one pane: {problems:?}"
    );
    let problem = &problems[0];
    assert!(
        problem.key.ends_with(&format!("/{pane}")),
        "the problem has to name the pane it is about, or a window of fifteen agents cannot \
         say which one went deaf: {problem:?}"
    );
    assert_eq!(
        problem.severity, "error",
        "an error is what opens a roster somebody closed. A warning waits to be found, and \
         being found by typing is the silence this exists to end: {problem:?}"
    );
    assert!(
        problem.detail.contains(&pane) && problem.detail.contains("channel.accept.failed"),
        "the sentence has to name the pane and where to look for the cause: {problem:?}"
    );
}

/// Every pane the last published view shows.
fn panes() -> Vec<String> {
    latest_view()
        .into_iter()
        .flat_map(|view| view.regions)
        .filter_map(|region| region.root)
        .flat_map(|root| leaves(&root))
        .collect()
}

fn leaves(node: &ViewNode) -> Vec<String> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => vec![pane.pane_id.clone()],
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);
static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
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

/// Waits for something a daemon has to say, rather than sleeping and hoping.
fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}. {}", on_failure());
}
