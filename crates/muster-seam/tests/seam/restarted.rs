//! A daemon that restarts under an open window, against a real daemon.
//!
//! It answers again, its tab comes back, and every check on the connection reads `connected`,
//! while every process in its panes is new. Only the window knows what was there before, so it
//! has to say so: in its problem list, on the machine's row in the agent list, and on its line in
//! `muster window` - the first two only while something it stopped still needs somebody.

use std::sync::Mutex;
use std::time::Duration;

use muster::proto::{
    Event, OpenWindow, ProblemsChanged, ReadWindow, Request, Response, Startup, event, request,
    response,
};
use muster_daemon_proto as proto;
use muster_harness::requests::{close_request, create, expect, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn a_restart_that_stopped_an_agent_is_said_until_its_pane_is_seen_to() {
    let _turn = muster::testing::fresh_session();
    // No bridge runs here, and a pane waiting for one would raise a problem of its own.
    muster::testing::set_typeable_deadline(Duration::ZERO);
    let mut daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to see the agent in p1",
        || format!("{:?}", read_window().roster).contains("claude"),
        || format!("the window's roster: {:?}", read_window().roster),
    );
    // Only what the daemon wrote down comes back, so the restart waits for p1 to be in its file.
    let state = daemon.root().join("daemon.state.json");
    until(
        "the daemon to write p1 down",
        || std::fs::read_to_string(&state).is_ok_and(|saved| saved.contains("\"p1\"")),
        || std::fs::read_to_string(&state).unwrap_or_default(),
    );
    assert!(restart_problem().is_none(), "nothing has restarted yet");

    daemon.kill();
    daemon.restart();

    until(
        "the window to say the daemon restarted",
        || restart_problem().is_some(),
        || format!("the problems the window lists: {:?}", latest_problems()),
    );
    let said = restart_problem().expect("just waited for it");
    assert_eq!(said.severity, "warning", "nothing is broken now; it should not take the roster");
    assert!(
        said.detail.contains(
            "restarted, so its pane came back from its saved state with new \
                              processes"
        ) && said.detail.contains("1 agent stopped and has to be started again"),
        "{}",
        said.detail
    );
    let cost = "restarted: 1 pane started again, 1 agent stopped";
    until(
        "muster window and the machine's row to say what the restart cost",
        || machine_line().is_some_and(|(detail, _)| detail == cost) && machine_row() == cost,
        || format!("the machines the window lists: {:?}", read_window()),
    );

    // The pane whose agent stopped is closed: nothing is left for the warning to ask about.
    expect(&mut daemon.connect(), close_request("p1"), proto::Outcome::Done);
    until(
        "the warning and the machine's row to go",
        || restart_problem().is_none() && machine_row().is_empty(),
        || format!("problems: {:?}\nmachines: {:?}", latest_problems(), read_window()),
    );
    let history = machine_line().map(|(detail, _)| detail).unwrap_or_default();
    assert_eq!(history, cost, "muster window keeps the machine's last restart");
}

/// What the machine's row in the agent list says about a restart: empty while it has no row
/// for one.
fn machine_row() -> String {
    read_window()
        .roster
        .and_then(|roster| roster.machines.into_iter().next())
        .map(|machine| machine.restarted)
        .unwrap_or_default()
}

/// The window's warning about a restart, if it has one.
fn restart_problem() -> Option<muster::proto::Problem> {
    latest_problems().into_iter().find(|problem| problem.key.starts_with("restarted:"))
}

/// The machine's detail and how many panes it holds, as `muster window` would print them.
fn machine_line() -> Option<(String, u32)> {
    read_window().daemons.first().map(|machine| (machine.detail.clone(), machine.panes))
}

fn read_window() -> muster::proto::Window {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ProblemsChanged(problems)) = event.payload {
        *PROBLEMS.lock().expect("a panicking test poisoned the problems") = Some(problems);
    }
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
