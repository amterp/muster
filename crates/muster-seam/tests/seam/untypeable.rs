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

use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;

use muster::proto::{
    Event, OpenWindow, ProblemsChanged, Request, Response, Startup, ViewChanged, ViewNode, event,
    request, response, view_node,
};
use muster_core::bridge_link::Report;
use muster_daemon_proto::{self as daemon_proto, pane_request};
use muster_harness::requests::{beside, create, expect, in_new_tab, make, pane, snapshot};
use muster_harness::{Control, Daemon, until};
use prost::Message;

/// Short enough that the gate does not wait out the shipped five seconds, and long enough that
/// it is still a deadline rather than an immediate accusation - the daemon has to answer, the
/// view has to be published, and a socket has to be bound before the clock even starts.
const DEADLINE_MS: u64 = 300;

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

    let daemon = Daemon::start_built();
    let config = daemon.muster_config();

    watch_events();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

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
        problem.detail.contains(&pane) && problem.detail.contains("link.accept.failed"),
        "the sentence has to name the pane and where to look for the cause: {problem:?}"
    );

    // A pane nothing has ever dialed offers a reattach beside the sentence, and the button is
    // the request itself: the shell sends it back as it came, so it has to work as it came.
    let remedy = problem.remedy.clone().unwrap_or_else(|| {
        panic!(
            "a pane no bridge ever dialed is one Muster asks for again on its own, so asking now \
             is safe to offer as a button: {problem:?}"
        )
    });
    assert_eq!(remedy.title, "Reattach");
    let sent = remedy.request.expect("a remedy carries the request its button sends");
    match &sent.payload {
        Some(request::Payload::ReattachPane(reattach)) => assert_eq!(reattach.pane_id, pane),
        other => panic!("the remedy should reattach {pane}, and it sends {other:?}"),
    }
    let before = restarts(&pane);
    let reply = Response::decode(muster::dispatch(&sent.encode_to_vec()).as_slice())
        .expect("the core answers with a response this build knows");
    assert_ok(&reply);
    until(
        "the remedy to ask the shell for a new bridge",
        || restarts(&pane) > before,
        || format!("the pane's bridge count stayed at {before}: {:?}", latest_view()),
    );
}

/// A window opening onto a zoomed tab accuses nobody, and unzooming it accuses nobody either.
///
/// A socket used to be bound for every leaf of the tab's tree while a zoomed region draws one
/// pane, so three sockets sat bound with nothing to dial them and the watch reported all three,
/// on every launch onto a zoomed tab and as a notification each. The panes were fine; nothing
/// was drawing them.
///
/// It took two halves of a fix to hold: binding a socket only for the pane a region draws, and
/// counting a wait only while the window is showing the pane. The second was needed because
/// herdr replayed a tab through the arrangements it had, one of them from before the zoom, so
/// the covered panes were briefly drawn and legitimately given sockets that then outlived the
/// drawing. muster-daemon's snapshot describes a tab only as it is now, and neither half has
/// been taken out.
///
/// Unzooming is the other direction, and the reason the narrowing is safe rather than merely
/// quiet: the revealed panes have to get their sockets before the shell is handed a view naming
/// them, or unzooming paints one typeable pane beside three the keyboard cannot reach.
#[test]
fn a_zoomed_tab_does_not_accuse_the_panes_it_covers() {
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();

    let daemon = Daemon::start_built();
    // Four panes in one tab with one filling it, arranged through the daemon's own protocol
    // before Muster has heard of any of it. That is a window reopening onto the tab somebody
    // left zoomed, which is the only way to reach this: a tab zoomed while Muster watches keeps
    // the sockets it already bound.
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for covered in ["p2", "p3", "p4"] {
        make(&mut control, create(covered, beside("p1", daemon_proto::Side::Down)));
    }
    zoom(&mut control, "p1", true);
    let held = daemon_panes(&mut control);
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
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the window to open onto the zoomed tab",
        || filling().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let (filling, socket) = filling().expect("just waited for one");
    assert!(
        !socket.is_empty(),
        "the pane filling the region has no link socket, so it would paint nothing and \
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
    // Proving a negative takes elapsed time (`docs/testing.md`). A socket bound for a covered
    // pane would have been bound no earlier than the drawn pane's, whose problem has already
    // arrived; herdr's replay once put the last of them 432ms later. Three deadlines outlasts
    // that with room, and is not a guess at how long a machine takes.
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

    zoom(&mut control, "p1", false);
    until(
        "the window to paint every pane the tab holds, each with a socket to dial",
        || {
            let painted = painted_panes();
            painted.len() == held.len() && painted.iter().all(|(_, socket)| !socket.is_empty())
        },
        || {
            format!(
                "unzooming left {:?}, and a pane published without a link socket is one the \
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

/// A bridge that dials in late takes the problem back, and the run log says both what the
/// person was told and why it went away.
///
/// The run log used to carry neither. Reconstructing an incident needed the events around the
/// problem and a guess, because the sentence a person read was the one thing the timeline did not
/// contain - and a clear could not say whether the pane had recovered or had only stopped being
/// looked at (kan a_2LMpvavhA, a_2LWqtPd8E).
#[test]
fn a_problem_and_its_clearing_are_in_the_run_log_with_why() {
    let _turn = muster::testing::fresh_session();
    shorten_the_deadline();

    let daemon = Daemon::start_built();
    let log = daemon.root().join("run.jsonl");
    watch_events();
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        log_path: log.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    until(
        "the window to show a pane with a socket",
        || painted_panes().iter().any(|(_, socket)| !socket.is_empty()),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let (pane, socket) = painted_panes().pop().expect("just waited for one");
    until(
        "the core to report the pane nothing has dialed",
        || !latest_problems().is_empty(),
        || format!("nothing was reported {DEADLINE_MS}ms after a socket was bound for {pane}"),
    );

    // Dialing and saying it attached is what a bridge does once the daemon has accepted its
    // stream, and the attach is what makes a pane typeable.
    let mut bridge = std::os::unix::net::UnixStream::connect(&socket)
        .expect("the core is listening on the pane's socket");
    bridge
        .write_all(Report::Attached.line().as_bytes())
        .expect("the core reads what a bridge says");
    until(
        "a bridge dialing in to take the problem back",
        || latest_problems().is_empty(),
        || format!("still outstanding after a dial: {:?}", latest_problems()),
    );

    let records = records(&log);
    let raised = records.iter().find(|record| record["event"] == "problem.raised");
    assert!(
        raised.is_some_and(|record| {
            record["key"].as_str().is_some_and(|key| key.ends_with(&format!("/{pane}")))
                && record["severity"] == "error"
                && record["detail"].as_str().is_some_and(|detail| detail.contains(&pane))
        }),
        "the run log should carry the problem a person was shown, sentence and all: {raised:?}"
    );
    let cleared = records.iter().find(|record| record["event"] == "problem.cleared");
    assert!(
        cleared.is_some_and(|record| {
            record["key"].as_str().is_some_and(|key| key.ends_with(&format!("/{pane}")))
                && record["why"] == "dialed"
        }),
        "the run log should say the problem went because a bridge dialed in: {cleared:?}"
    );
}

/// Every record the run so far has written, read back as JSON.
fn records(log: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .expect("the run log was opened")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Sets the deadline this test runs under, before any pane opens.
fn shorten_the_deadline() {
    muster::testing::set_typeable_deadline(Duration::from_millis(DEADLINE_MS));
}

fn zoom(control: &mut Control, pane_name: &str, zoomed: bool) {
    expect(
        control,
        pane(pane_request::Request::Zoom(pane_request::Zoom {
            pane: pane_name.to_string(),
            zoomed,
        })),
        daemon_proto::Outcome::Done,
    );
}

fn daemon_panes(control: &mut Control) -> Vec<String> {
    snapshot(control).panes.into_iter().map(|pane| pane.pane).collect()
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
        view_node::Node::Pane(pane) => Some((pane.pane_id, pane.link_socket_path)),
        view_node::Node::Split(_) => None,
    }
}

/// How many bridges the last published view has asked for a pane, which is what moves when
/// somebody asks for one.
fn restarts(pane_id: &str) -> u32 {
    fn find(node: &ViewNode, pane_id: &str) -> Option<u32> {
        match &node.node {
            Some(view_node::Node::Pane(pane)) => {
                (pane.pane_id == pane_id).then_some(pane.bridge_restarts)
            }
            Some(view_node::Node::Split(split)) => {
                split.first.iter().chain(split.second.iter()).find_map(|child| find(child, pane_id))
            }
            None => None,
        }
    }
    latest_view()
        .into_iter()
        .flat_map(|view| view.regions)
        .filter_map(|region| region.root)
        .find_map(|root| find(&root, pane_id))
        .unwrap_or_default()
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
            vec![(pane.pane_id.clone(), pane.link_socket_path.clone())]
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
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
