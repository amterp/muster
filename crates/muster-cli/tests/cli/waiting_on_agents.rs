//! Whether a caller can wait on an agent through the command it types, instead of polling.
//!
//! The real binary against a real window and daemon, for what `muster-seam/tests/seam/agent_states.rs`
//! cannot see from inside the process: that a watch's lines reach a pipe as they happen rather
//! than when the command exits, and that a wait's exit code is the one a script branches on
//! (kan a_2M9T8O6dL).
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives: this suite runs
//! inside Muster, and an inherited `MUSTER_SOCKET` is the developer's own window.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_harness::{Daemon, PATIENCE, until, until_some};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn a_caller_can_wait_on_an_agent_instead_of_polling() {
    let _turn = muster::testing::fresh_session();
    let mut open = a_window_onto_one_pane();

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

    // The daemon decides a pane's state from what runs in it and what it paints, so every
    // gesture after this one needs an agent there to change it.
    open.daemon.run_agent(&open.pane);

    a_watch_prints_each_change_as_it_happens(&open);
    a_wait_exits_when_the_agent_finishes(&open);
    a_layout_watch_draws_again_when_the_arrangement_moves(&open);
    a_pane_just_made_can_be_waited_on(&open);

    let ran = muster(&open, &["pane", "wait", "--pane", "p1nobody00", "--until", "idle"]);
    assert_eq!(
        ran.status.code(),
        Some(1),
        "a wait on a pane nobody holds can never end, and has to be refused rather than hang: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    a_daemon_that_stops_answering_is_heard_by_a_watch_and_ends_a_wait(&mut open);
}

/// A daemon that stops answering is a line on a watch, and ends a wait on its pane with exit 4.
///
/// Last, because it kills the daemon every gesture before it needs. 4 rather than 5: 5 tells a
/// script the agent is still going and waiting again is harmless, and while its daemon is stale
/// nobody can say what the agent is doing.
fn a_daemon_that_stops_answering_is_heard_by_a_watch_and_ends_a_wait(open: &mut Open) {
    let mut watch = spawned(open, &["--json", "window", "--watch"]);
    let lines = lines_of(&mut watch);
    let first = next_json(&lines, "the watch's first line, which is a pane as it stands");

    let wait = command(
        open,
        &["pane", "wait", "--pane", &open.pane, "--until", "blocked", "--timeout", "60"],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"));
    until(
        "the window to hold the watch and the wait open",
        || muster::testing::watchers() == 2,
        || format!("{} watches are open", muster::testing::watchers()),
    );

    open.daemon.kill();
    // Past the rest of the panes as they stood, which are also on this daemon.
    let stale = loop {
        let line = next_json(&lines, "the watch to print the daemon going stale");
        if line.get("pane").is_none() {
            break line;
        }
    };
    assert!(
        stale["daemon"] == first["daemon"] && stale["state"] == json!("stale"),
        "a daemon that stopped answering has to be a line of its own, naming the daemon: {stale}"
    );

    let ended = wait.wait_with_output().expect("the wait was spawned by this test");
    let said = String::from_utf8_lossy(&ended.stderr);
    assert_eq!(
        ended.status.code(),
        Some(4),
        "a wait whose daemon stopped answering exits {:?}, so a script cannot tell an agent still \
         working from one nobody can see: {said}",
        ended.status.code()
    );
    assert!(said.contains(&open.pane), "a wait that lost its daemon should name its pane: {said}");

    let _ = watch.kill();
    let _ = watch.wait();
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
    assert!(
        first["label"].as_str().is_some_and(|label| !label.is_empty()),
        "a watch line says what to call its pane, as `muster window` does: {first}"
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

/// A pane is waited on straight after it is made.
///
/// `muster pane wait --pane "$(muster pane new)"` is the line a script writes, and it asks about
/// the pane in the same instant `pane new` names it. The window holds the daemon's description of
/// a new pane before it answers the request that made it; a wait refusing the name would mean
/// that order had broken, and the caller would lose a race it cannot see.
/// `window --watch --layout --json` prints the layout, then the layout again each time the
/// arrangement moves, and nothing for an agent changing state.
///
/// The negative is read off the order of the lines rather than timed: the state changes before
/// the split, so a drawing printed for it would be the next line, and the next line is the split.
fn a_layout_watch_draws_again_when_the_arrangement_moves(open: &Open) {
    let mut watch = spawned(open, &["--json", "window", "--watch", "--layout"]);
    let lines = lines_of(&mut watch);
    let panes = |drawn: &Value| drawn["panes"].as_array().map_or(0, Vec::len);

    let first = next_json(&lines, "the layout as it stands");
    assert_eq!(panes(&first), 1, "the layout begins as the window stands: {first}");
    assert!(first["panes"][0]["frame"].is_object(), "a drawing carries the layout: {first}");

    open.report("working");
    let made = muster(open, &["pane", "new", "--pane", &open.pane, "--down"]);
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let split = next_json(&lines, "the layout drawn again after a split");
    assert_eq!(
        panes(&split),
        2,
        "the next drawing after a state change and a split is the split, so the state change \
         drew nothing: {split}"
    );

    let _ = watch.kill();
    let _ = watch.wait();
    let closed = muster(
        open,
        &["pane", "close", "--pane", split["panes"][1]["pane"].as_str().unwrap_or_default()],
    );
    assert!(closed.status.success(), "{}", String::from_utf8_lossy(&closed.stderr));
}

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
}

impl Open {
    /// Tells the fake agent in the pane what to paint, through `muster pane send` as another
    /// agent would, and waits until the window says the pane is in that state.
    ///
    /// Waited for here rather than left to the gesture, because the daemon takes a moment to
    /// read a new screen, and a wait spawned in that moment would see the state before. An
    /// agent told to go idle after working is `done` to the window, which is Muster's name for
    /// an idle agent nobody has looked at since, so either answers that.
    fn report(&self, state: &str) {
        let sent = muster(self, &["pane", "send", "--pane", &self.pane, state, "--enter"]);
        assert!(
            sent.status.success(),
            "`muster pane send` could not tell the agent to be {state}: {}",
            String::from_utf8_lossy(&sent.stderr)
        );
        until(
            &format!("the window to say {} is {state}", self.pane),
            || self.state().is_some_and(|now| now == state || (state == "idle" && now == "done")),
            || format!("the window says {} is {:?}", self.pane, self.state()),
        );
    }

    /// The pane's state as `muster window` says it.
    fn state(&self) -> Option<String> {
        let ran = muster(self, &["--json", "window"]);
        let window: Value = serde_json::from_slice(&ran.stdout).ok()?;
        let pane =
            window["panes"].as_array()?.iter().find(|pane| pane["pane"] == json!(self.pane))?;
        Some(pane["state"].as_str()?.to_string())
    }
}

/// A window onto an empty daemon, which asks it for a first tab: that tab's pane is the one
/// this test waits on.
fn a_window_onto_one_pane() -> Open {
    let daemon = Daemon::start_detecting();

    let socket = daemon.root().join("command.sock").to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow::default())));

    let mut open = Open { daemon, socket, pane: String::new() };
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
    let reply = muster::dispatch(&Request::new(payload).encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
