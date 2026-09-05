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
//! And its opposite, which matters as much. An alarm that fires on a healthy window is what
//! teaches somebody to ignore the one that does not, so the false positive is covered beside
//! the true one rather than somewhere else.
//!
//! Its own binary because this needs a process whose deadline was set before any pane opened,
//! and the deadline is read once per process. The seam serializes the tests inside a binary.

use std::sync::{Mutex, Once};
use std::time::Duration;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, ProblemsChanged, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use prost::Message;
use serde_json::json;

/// Short enough that the gate does not wait out the shipped five seconds, and long enough that
/// it is still a deadline rather than an immediate accusation - the daemon has to answer, the
/// view has to be published, and a socket has to be bound before the clock even starts.
const DEADLINE_MS: &str = "300";

/// How long "and nothing else was reported" waits before it counts as true.
///
/// Three deadlines. There is no event for nothing further arriving, so a negative costs
/// elapsed time by construction (`docs/testing.md`) - and the measurement behind the number is
/// in the test that uses it.
const SETTLE: Duration = Duration::from_millis(900);

#[test]
fn a_pane_whose_bridge_never_dials_is_reported() {
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();

    let daemon = Daemon::start();
    let config = daemon.muster_config();

    watch_events();
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

/// A window opening onto a zoomed tab accuses nobody, and unzooming it accuses nobody either.
///
/// A socket used to be bound for every leaf of the tab's tree while a zoomed region draws one
/// pane, so three sockets sat bound with nothing to dial them and the watch reported all three,
/// on every launch onto a zoomed tab and as a notification each. The panes were fine; nothing
/// was drawing them.
///
/// It takes both halves of the fix to hold, which is why the test is worth its daemon. Binding
/// a socket only for the pane a region draws is not enough on its own: herdr's bootstrap replay
/// walks the tab through the arrangements it had, one of them from before the zoom, so the
/// covered panes are briefly drawn and legitimately given sockets that then outlive the
/// drawing. What settles it is that the watch counts a wait only while the window is showing
/// the pane.
///
/// Unzooming is the other direction, and the reason the narrowing is safe rather than merely
/// quiet: the revealed panes have to get their sockets before the shell is handed a view naming
/// them, or unzooming paints one typeable pane beside three the keyboard cannot reach.
#[test]
fn a_zoomed_tab_does_not_accuse_the_panes_it_covers() {
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();

    let daemon = Daemon::start();
    // Four panes in one tab with one filling it, arranged through herdr's own API before
    // Muster has heard of any of it. That is a window reopening onto the tab somebody left
    // zoomed, which is the only way to reach this: a tab zoomed while Muster watches keeps
    // the sockets it already bound.
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "zoomed", "focus": true }));
    let first = the_only_pane(&daemon);
    for _ in 0..3 {
        daemon.call("pane.split", &json!({ "target_pane_id": first, "direction": "down" }));
    }
    daemon.call("pane.zoom", &json!({ "pane_id": first, "mode": "on" }));
    let held = daemon_panes(&daemon);
    assert_eq!(
        held.len(),
        4,
        "the arrangement did not take, so what follows would be a test about one pane: {held:?}"
    );

    watch_events();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to open onto the zoomed tab",
        || filling().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let (filling, socket) = filling().expect("just waited for one");
    assert!(
        !socket.is_empty(),
        "the pane filling the region has no control socket, so it would paint nothing and \
         swallow the keyboard"
    );

    until(
        "the core to report the one pane nothing has dialed",
        || !latest_problems().is_empty(),
        || {
            format!(
                "nothing was reported {DEADLINE_MS}ms after a socket was bound for {filling}. \
                 The pane on screen is genuinely deaf here, so this half has to fire before \
                 the half below can mean anything"
            )
        },
    );
    // Proving a negative takes elapsed time, and this is the measurement behind the number
    // (`docs/testing.md`). The covered panes get sockets of their own here, because herdr's
    // bootstrap replay walks this tab through the arrangements it had and one of them is the
    // tab before it was zoomed - measured at 432ms between the drawn pane's socket and the
    // last of theirs, so their deadlines expire that much later than the problem waited for
    // above. Three deadlines outlasts the last of them with room, and is not a guess at how
    // long a machine takes.
    std::thread::sleep(SETTLE);
    let problems = latest_problems();
    assert_eq!(
        problems.len(),
        1,
        "a zoomed tab of four panes raised {} problems. Three of its panes have no surface \
         because nothing is drawing them, and a socket bound for one of those is an alarm on \
         a healthy window - which is what teaches somebody to ignore the alarm that matters: \
         {problems:?}",
        problems.len()
    );
    assert!(
        problems[0].key.ends_with(&format!("/{filling}")),
        "the one problem names a pane other than the one on screen: {problems:?}"
    );

    daemon.call("pane.zoom", &json!({ "pane_id": first, "mode": "off" }));
    until(
        "the window to paint every pane the tab holds, each with a socket to dial",
        || {
            let painted = painted_panes();
            painted.len() == held.len() && painted.iter().all(|(_, socket)| !socket.is_empty())
        },
        || {
            format!(
                "unzooming left {:?}, and a pane published without a control socket is one the \
                 shell must not start a bridge for - so it paints nothing while the pane beside \
                 it takes the keyboard",
                painted_panes()
            )
        },
    );

    until(
        "the watch to report all four, now that all four have a socket nobody dialed",
        || latest_problems().len() == held.len(),
        || {
            format!(
                "the panes the unzoom revealed are as deaf as the one that was on screen, and \
                 saying so is the whole of this feature: {:?}",
                latest_problems()
            )
        },
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

/// The pane a new workspace comes with, read back rather than spelled.
///
/// Pane ids are the daemon's to hand out, and a test naming one would be asserting herdr's
/// numbering rather than Muster's behavior.
fn the_only_pane(daemon: &Daemon) -> String {
    let held = daemon_panes(daemon);
    match held.as_slice() {
        [only] => only.clone(),
        _ => panic!("a new workspace should hold exactly one pane, and this one holds {held:?}"),
    }
}

fn daemon_panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot["snapshot"]["panes"]
        .as_array()
        .map(|panes| {
            panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default()
}

/// The one pane a zoomed region is showing, and the socket a bridge for it would dial.
///
/// `None` while no region is zoomed, and for a zoomed region published as a split - which is
/// the zoom not having been resolved at all rather than an answer.
fn filling() -> Option<(String, String)> {
    let region = latest_view()?.regions.into_iter().next()?;
    if !region.zoomed {
        return None;
    }
    match region.root?.node? {
        view_node::Node::Pane(pane) => Some((pane.pane_id, pane.control_socket_path)),
        view_node::Node::Split(_) => None,
    }
}

/// Every pane the last published view shows.
fn panes() -> Vec<String> {
    painted_panes().into_iter().map(|(pane, _)| pane).collect()
}

/// The same, with the socket each one's bridge would dial.
fn painted_panes() -> Vec<(String, String)> {
    latest_view()
        .into_iter()
        .flat_map(|view| view.regions)
        .filter_map(|region| region.root)
        .flat_map(|root| leaves(&root))
        .collect()
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

/// Starts recording what the core says, forgetting what it said to the test before.
///
/// These two are this file's rather than the core's, so a session reset does not touch them -
/// and a test that read them without clearing would assert against the window before it. That
/// is not hypothetical: the first pane test read four problems belonging to the zoomed one.
fn watch_events() {
    *VIEW.lock().expect("a panicking test poisoned the view") = None;
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note));
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
