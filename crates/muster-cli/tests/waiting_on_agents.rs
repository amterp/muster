//! Whether a caller can wait on an agent through the command it types, instead of polling.
//!
//! The real binary against a real window and daemon, for what `muster-seam/tests/agent_states.rs`
//! cannot see from inside the process: that a watch's lines reach a pipe as they happen rather
//! than when the command exits, and that a wait's exit code is the one a script branches on
//! (kan a_2M9T8O6dL).
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives: this suite runs
//! inside Muster, and an inherited `MUSTER_SOCKET` is the developer's own window.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};

use herdr_harness::{Daemon, PATIENCE, until, until_some};
use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_caller_can_wait_on_an_agent_instead_of_polling() {
    let _turn = muster::testing::fresh_session();
    let open = a_window_onto_one_pane();

    let ran = muster(
        &open,
        &["pane", "wait", "--pane", &open.pane, "--until", "blocked", "--timeout", "1"],
    );
    assert_eq!(
        ran.status.code(),
        Some(5),
        "a plain shell is never blocked, so this wait has to run out and say so with 5: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    a_watch_prints_each_change_as_it_happens(&open);
    a_wait_exits_when_the_agent_finishes(&open);
    a_pane_just_made_can_be_waited_on(&open);

    let ran = muster(&open, &["pane", "wait", "--pane", "p1nobody00", "--until", "idle"]);
    assert_eq!(
        ran.status.code(),
        Some(1),
        "a wait on a pane nobody holds can never end, and has to be refused rather than hang: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

/// `window --watch --json` writes a line per pane, then a line per change, while it is running.
fn a_watch_prints_each_change_as_it_happens(open: &Open) {
    let mut watch = spawned(open, &["--json", "window", "--watch"]);
    let lines = lines_of(&mut watch);

    let first = next_json(&lines, "the watch's first line, which is the pane as it stands");
    assert!(
        first["pane"] == json!(open.pane) && first["since"].is_number(),
        "a watch has to begin with each pane and since when it has been in its state: {first}"
    );

    open.report("working");
    let working = next_json(&lines, "the watch to print the agent starting work");
    assert_eq!(
        (&working["pane"], &working["state"]),
        (&json!(open.pane), &json!("working")),
        "the line after the pane as it stood has to be the change that was made: {working}"
    );

    let _ = watch.kill();
    let _ = watch.wait();
    // Let go of before the next gesture counts watches, so the one it counts is its own.
    until(
        "the window to let go of the watch whose caller was killed",
        || muster::testing::watchers() == 0,
        || format!("{} watches are still open", muster::testing::watchers()),
    );
}

/// `pane wait --until idle` blocks while the agent works, and exits 0 naming the pane when it stops.
fn a_wait_exits_when_the_agent_finishes(open: &Open) {
    open.report("working");
    let mut wait = spawned(
        open,
        &["pane", "wait", "--pane", &open.pane, "--until", "idle", "--timeout", "60"],
    );
    until(
        "the window to hold the wait open",
        || muster::testing::watchers() == 1,
        || format!("{} watches are open", muster::testing::watchers()),
    );

    open.report("idle");
    let lines = lines_of(&mut wait);
    let said = lines
        .recv_timeout(PATIENCE)
        .unwrap_or_else(|_| panic!("`muster pane wait` printed nothing after the agent finished"));
    let status = wait.wait().expect("the wait was spawned by this test");
    assert_eq!(
        status.code(),
        Some(0),
        "a wait that got what it asked for exits {:?}, so a script's `&&` never runs",
        status.code()
    );
    // `done`, not `idle`: nothing told this window it has focus, and `idle` is met by either.
    assert_eq!(
        said,
        format!("{}  done", open.pane),
        "a wait prints the pane that got there and the state it is in, and nothing else"
    );
}

/// A pane is waited on straight after it is made, which is before the window has heard of it.
///
/// `pane new` answers with the name as soon as the daemon has made the pane, and the daemon's
/// event describing it reaches the window a moment later. A wait refusing that name would make
/// `muster pane wait --pane "$(muster pane new)"` a race the caller loses.
fn a_pane_just_made_can_be_waited_on(open: &Open) {
    let made = muster(open, &["pane", "new", "--pane", &open.pane, "--down"]);
    let made = String::from_utf8_lossy(&made.stdout).trim().to_string();
    let every_state = "working,blocked,idle,done,unknown";
    let ran =
        muster(open, &["pane", "wait", "--pane", &made, "--until", every_state, "--timeout", "10"]);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "a pane made a moment ago could not be waited on: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

struct Open {
    daemon: Daemon,
    socket: String,
    pane: String,
    backend: String,
}

impl Open {
    fn report(&self, state: &str) {
        self.daemon.call(
            "pane.report_agent",
            &json!({ "pane_id": self.backend, "agent": "probe", "source": "probe", "state": state }),
        );
    }
}

fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "waiting", "focus": true }));
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let backend = snapshot["snapshot"]["panes"][0]["pane_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a fresh workspace holds a pane: {snapshot}"))
        .to_string();

    let socket = daemon.root().join("command.sock").to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow {})));

    let mut open = Open { daemon, socket, pane: String::new(), backend };
    open.pane = until_some("the window to describe the pane the daemon holds", || {
        let ran = muster(&open, &["--json", "window"]);
        let window: Value = serde_json::from_slice(&ran.stdout).ok()?;
        Some(window["panes"].get(0)?["pane"].as_str()?.to_string())
    });
    open
}

fn command(open: &Open, argv: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_muster"));
    command.args(argv).env_clear().env("MUSTER_SOCKET", &open.socket);
    command
}

fn muster(open: &Open, argv: &[&str]) -> Output {
    command(open, argv)
        .output()
        .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"))
}

fn spawned(open: &Open, argv: &[&str]) -> Child {
    command(open, argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"))
}

/// A child's stdout, a line at a time as it is written.
fn lines_of(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().expect("stdout was asked to be piped");
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    lines
}

fn next_json(lines: &Receiver<String>, what: &str) -> Value {
    let line = lines.recv_timeout(PATIENCE).unwrap_or_else(|_| {
        panic!("timed out waiting for {what}: nothing reached the pipe, so a reader acting on each line hears nothing")
    });
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("a watch line under --json is not JSON ({error}): {line:?}"))
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request { payload: Some(payload) }.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
