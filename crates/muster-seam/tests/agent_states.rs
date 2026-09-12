//! What a caller outside the window can learn about what its agents are doing, and when.
//!
//! A snapshot answering `working` twice cannot say whether that was one turn or two with a finish
//! between them, and a caller that wants to know when an agent finishes has had nothing to do but
//! poll (kan a_2M9T8O6dL). These tests are against a real daemon, which is where agent states
//! come from, and they drive states through herdr's own `pane.report_agent`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use herdr_harness::{Daemon, until_some};
use muster::proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster::proto::{
    OpenWindow, PaneStateChanged, ReadWindow, Request, Response, Startup, Window, WindowFocus,
    request, response,
};
use prost::Message;
use serde_json::{Value, json};

/// A pane's agent says since when it has been doing what it is doing, and a look does not move it.
///
/// The clock belongs to the agent rather than to the window. A `done` pane somebody looks at
/// becomes `idle`, and it has been resting since it finished rather than since it was noticed -
/// so an integrator reading how long a worker has been idle reads how long its work has been
/// waiting.
#[test]
fn a_pane_says_since_when_its_agent_has_been_doing_it() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let first_seen = agent(&open.socket, &open.pane).since_ms;
    assert!(
        first_seen > 0,
        "a pane the window has seen carries no time at all, so a caller cannot say how long it \
         has been in its state: {:?}",
        agent(&open.socket, &open.pane)
    );

    let before = now_ms();
    open.report("working");
    let working = until_state(&open, "working");
    let after = now_ms();
    assert!(
        (before..=after).contains(&working.since_ms),
        "the agent started working between {before} and {after}, and the window says it has been \
         working since {} - a caller subtracting that from now gets a turn of the wrong length",
        working.since_ms
    );

    // Nothing has told the window it has focus, so a finish nobody saw is `done`.
    open.report("idle");
    let finished = until_state(&open, "done");
    assert!(
        finished.since_ms >= working.since_ms,
        "the agent finished at {} and started at {}, so the window's clock ran backwards",
        finished.since_ms,
        working.since_ms
    );

    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: true })));
    let seen = until_state(&open, "idle");
    assert_eq!(
        seen.since_ms, finished.since_ms,
        "looking at a finished pane restarted how long it has been resting, so an idle worker \
         reads as having finished the moment somebody glanced at it"
    );
}

/// One window, showing one pane, whose herdr and Muster names are both known.
struct Open {
    daemon: Daemon,
    socket: PathBuf,
    /// What Muster calls the pane, which is what every request addresses.
    pane: String,
    /// What the daemon calls it, which is what `pane.report_agent` addresses.
    backend: String,
}

impl Open {
    /// Tells the daemon the pane's agent is in `state`, the way a harness hook would.
    fn report(&self, state: &str) {
        self.daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": self.backend, "agent": "probe", "source": "probe", "state": state }),
        );
    }
}

fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "agents", "focus": true }));
    let backend = only_pane(&daemon);
    // Named before startup so this test can find it again: Muster mints its own name for the
    // pane, and herdr announces a rename to nobody, so the bootstrap snapshot is the only thing
    // that carries one (`observations/herdr-0.8.0.md` section 16).
    daemon.call("pane.rename", &json!({ "pane_id": backend, "label": "agent" }));

    let socket = daemon.root().join("command.sock");
    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    let pane = until_some("the window to list the pane called agent", || {
        read_window(&socket)
            .roster
            .iter()
            .flat_map(|roster| roster.tabs.iter())
            .flat_map(|tab| tab.panes.iter())
            .find(|pane| pane.given_name == "agent")
            .map(|pane| pane.pane_id.clone())
    });
    Open { daemon, socket, pane, backend }
}

/// What the window says one pane's agent is doing, asked over the socket a CLI dials.
fn agent(socket: &std::path::Path, pane: &str) -> PaneStateChanged {
    let window = read_window(socket);
    window
        .panes
        .iter()
        .find(|agent| agent.pane_id == pane)
        .cloned()
        .unwrap_or_else(|| panic!("the window lists no agent for {pane}: {window:?}"))
}

fn until_state(open: &Open, state: &str) -> PaneStateChanged {
    until_some(&format!("the window to say the agent is {state}"), || {
        Some(agent(&open.socket, &open.pane)).filter(|agent| agent.state == state)
    })
}

fn read_window(socket: &std::path::Path) -> Window {
    match dialed(socket, request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("the endpoint answered a ReadWindow with {other:?}"),
    }
}

/// One request over one connection, and its one answer.
fn dialed(socket: &std::path::Path, payload: request::Payload) -> Response {
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .unwrap_or_else(|error| panic!("nothing is listening on {}: {error}", socket.display()));
    write_frame(&mut stream, &Request { payload: Some(payload) }.encode_to_vec())
        .expect("the endpoint takes a request");
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).expect("the endpoint answers it");
    Response::decode(reply.as_slice()).expect("the answer is a response this build knows")
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes: Vec<String> = snapshot["snapshot"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

fn now_ms() -> i64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is past 1970");
    i64::try_from(since.as_millis()).expect("milliseconds since 1970 fit an i64")
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request { payload: Some(payload) }.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
