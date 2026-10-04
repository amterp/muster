//! The pane verbs with no window answering: `muster window`, `pane read`, `pane send` and
//! `pane wait`, answered by the daemon holding the panes, as they are on a devenv nothing
//! forwards a window to, or in a pane whose window has quit. The real binary against a real
//! daemon built from this commit, with `$MUSTER_SOCKET` naming a window that is not there.
//!
//! The child's environment is cleared for the reason `driving_a_window.rs` gives: this suite runs
//! inside Muster, and an inherited `MUSTER_SOCKET` is the developer's own window.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_daemon_proto::{self as proto, session_request};
use muster_harness::requests::{
    beside, close_request, create, expect, in_new_tab, make, pane, session, until_text,
};
use muster_harness::{Daemon, PATIENCE, until_some};
use prost::Message;
use serde_json::{Value, json};

/// Where a caller finds itself: a daemon, and a window it was told about that has gone.
struct Here {
    daemon: Daemon,
    /// `$MUSTER_SOCKET`: nothing listens on it.
    window: String,
    /// `$MUSTER_HOME`, with no window's socket in its state directory.
    home: PathBuf,
}

impl Here {
    fn new(daemon: Daemon) -> Here {
        let home = daemon.root().join("no-window-here");
        std::fs::create_dir_all(home.join("state")).unwrap();
        let window = daemon.root().join("quit.sock").to_string_lossy().into_owned();
        Here { daemon, window, home }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_muster"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("MUSTER_HOME", &self.home)
            .env("MUSTER_SOCKET", &self.window)
            .env("MUSTER_DAEMON_SOCKET", self.daemon.socket_path())
            .stdin(Stdio::null());
        command
    }

    fn muster(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().expect("the muster binary runs")
    }

    fn spawned(&self, arguments: &[&str]) -> Child {
        self.command(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the muster binary runs")
    }
}

fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

fn complained(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim_end().to_string()
}

#[track_caller]
fn ok(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", complained(output));
    said(output)
}

#[track_caller]
fn refused_with(output: &Output, code: i32) -> String {
    assert_eq!(output.status.code(), Some(code), "{}\n{}", said(output), complained(output));
    complained(output)
}

/// A daemon holding a shell in `p1` in tab `t1`, and the fake agent in `p2` and `p3` beside it.
fn panes() -> Here {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    for pane in ["p2", "p3"] {
        make(&mut control, create(pane, beside("p1", proto::Side::Right)));
    }
    for pane in ["p2", "p3"] {
        daemon.run_agent(pane);
    }
    until_text(&mut control, "p1", "$");
    Here::new(daemon)
}

#[test]
fn with_no_window_the_daemon_lists_reads_types_into_and_waits_on_its_panes() {
    let here = panes();

    let listed = ok(&here.muster(&["window"]));
    assert!(
        listed.starts_with("no window answered; the muster-daemon at")
            && listed.contains(&here.daemon.socket_path().display().to_string()),
        "the listing has to say which daemon answered, so nobody reads it as a window's:\n{listed}"
    );
    assert!(listed.contains("t1") && listed.contains("p1") && listed.contains("p2"), "{listed}");
    assert!(!listed.contains("(hidden)"), "with no window, nothing is hidden from one:\n{listed}");

    let listed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    assert_eq!(listed["answered_by"], json!("daemon"), "{listed}");
    let p2 = listed["panes"].as_array().unwrap().iter().find(|pane| pane["pane"] == json!("p2"));
    let p2 = p2.unwrap_or_else(|| panic!("p2 is listed: {listed}"));
    assert_eq!((&p2["tab"], &p2["state"]), (&json!("t1"), &json!("idle")), "{listed}");
    the_layout_is_the_daemons_trees(&here);

    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", "echo typed-in"]));
    let mut control = here.daemon.connect();
    until_text(&mut control, "p1", "typed-in\n");
    let read = ok(&here.muster(&["pane", "read", "--pane", "p1"]));
    assert!(read.contains("echo typed-in"), "the read shows what the pane printed:\n{read}");
    let last: Value = serde_json::from_str(&ok(
        &here.muster(&["--json", "pane", "read", "--pane", "p1", "--rows", "1"])
    ))
    .unwrap();
    assert_eq!((&last["rows"], &last["truncated"]), (&json!(1), &json!(true)), "{last}");

    a_wait_ends_when_the_agent_gets_there(&here);
    a_wait_on_context_ends_when_the_agent_says_it(&here);
    a_compaction_is_asked_of_the_daemon(&here);
    a_watch_prints_each_change(&here);
    a_wait_on_a_pane_that_closes_is_refused(&here);
    a_send_the_pane_never_shows_is_refused(&here);

    let refused = refused_with(&here.muster(&["pane", "read"]), 1);
    assert!(refused.contains("--pane"), "with no window there is no keyboard's pane:\n{refused}");
    let refused = refused_with(&here.muster(&["pane", "read", "--pane", "p9nobody"]), 1);
    assert!(refused.contains("no pane called p9nobody"), "{refused}");
    // A shell, whose agent never went to work: the daemon was asked for the turn, and said so.
    let refused = refused_with(&here.muster(&["pane", "read", "--pane", "p1", "--turn"]), 1);
    assert!(refused.contains("no turn has started"), "{refused}");
}

/// With no window, `--layout` draws each tab from the daemon's own tree, places each pane in it as
/// a window would, and asks the daemon for the sizes.
fn the_layout_is_the_daemons_trees(here: &Here) {
    let laid: Value =
        serde_json::from_str(&ok(&here.muster(&["--json", "window", "--layout"]))).unwrap();
    let layout = &laid["tabs"][0]["regions"][0]["layout"];
    assert_eq!(layout["axis"], json!("columns"), "p2 and p3 went to p1's right: {laid}");
    let mut widths = 0.0;
    for pane in laid["panes"].as_array().into_iter().flatten() {
        assert!(pane["cells"]["cols"].as_u64().is_some_and(|cols| cols > 0), "{pane}");
        assert_eq!(
            pane["frame"]["height"],
            json!(1.0),
            "side by side, each the full height: {pane}"
        );
        widths += pane["frame"]["width"].as_f64().unwrap_or_default();
    }
    assert!((widths - 1.0).abs() < 1e-4, "the three frames share the tab's width: {laid}");
    let drawn = ok(&here.muster(&["window", "--layout"]));
    assert!(drawn.contains('┬') && drawn.contains("p3"), "three panes side by side:\n{drawn}");
}

/// A wait for a state the pane is already in ends at once; one for a state it reaches later ends
/// when it does, naming the pane; and `idle` is met by the `done` a finish leaves.
fn a_wait_ends_when_the_agent_gets_there(here: &Here) {
    assert_eq!(ok(&here.muster(&["pane", "wait", "--pane", "p2", "--until", "idle"])), "p2  idle");

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--until", "working", "--timeout", "60"]);
    here.daemon.set_agent_state("p2", proto::AgentState::Working);
    let waited = wait.wait_with_output().unwrap();
    assert_eq!(ok(&waited), "p2  working");

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--until", "idle", "--timeout", "60"]);
    here.daemon.set_agent_state("p2", proto::AgentState::Idle);
    let waited = ok(&wait.wait_with_output().unwrap());
    assert!(
        waited == "p2  done" || waited == "p2  idle",
        "an agent that finished with nobody looking is done, which a wait for idle accepts: {waited}"
    );

    let ran =
        here.muster(&["pane", "wait", "--pane", "p2", "--until", "blocked", "--timeout", "1"]);
    refused_with(&ran, 5);
}

/// A wait on context says at once that the pane has not said its context, ends when the agent
/// says it is that full, and prints how full beside the state.
fn a_wait_on_context_ends_when_the_agent_says_it(here: &Here) {
    let ran = here.muster(&["pane", "wait", "--pane", "p2", "--context", "80", "--timeout", "1"]);
    let complaint = refused_with(&ran, 5);
    assert!(
        complaint.contains("p2 has not said how full its context is"),
        "a wait its harness may never meet has to say so before the timeout does:\n{complaint}"
    );
    assert!(complaint.contains("not at 80% context within 1s"), "{complaint}");

    let wait =
        here.spawned(&["pane", "wait", "--pane", "p2", "--context", "80", "--timeout", "60"]);
    let report = proto::pane_request::Report {
        pane: "p2".to_string(),
        context_used: Some(85.0),
        ..Default::default()
    };
    expect(
        &mut here.daemon.connect(),
        pane(proto::pane_request::Request::Report(report)),
        proto::Outcome::Done,
    );
    // `done`: the wait before this one left a finish nobody has looked at.
    assert_eq!(ok(&wait.wait_with_output().unwrap()), "p2  done  85% context");
}

/// With no window, `pane compact` asks the daemon, and a shell, which has no context, is refused.
fn a_compaction_is_asked_of_the_daemon(here: &Here) {
    ok(&here.muster(&["pane", "compact", "--pane", "p3"]));
    let heard = here.daemon.root().join("home/fake-agent-heard");
    until_some("the agent in p3 to read the compaction", || {
        std::fs::read_to_string(&heard).ok()?.lines().any(|line| line == "/compact").then_some(())
    });
    let refused = refused_with(&here.muster(&["pane", "compact", "--pane", "p1"]), 1);
    assert!(refused.contains("runs no agent"), "{refused}");
}

/// `window --watch --json` says each pane as it stands, then each change.
fn a_watch_prints_each_change(here: &Here) {
    let mut watch = here.spawned(&["--json", "window", "--watch"]);
    let lines = lines_of(&mut watch);
    let mut first = Vec::new();
    for _ in 0..3 {
        first.push(next_json(&lines, "a pane as it stands")["pane"].clone());
    }
    first.sort_by_key(ToString::to_string);
    assert_eq!(first, [json!("p1"), json!("p2"), json!("p3")]);

    here.daemon.set_agent_state("p3", proto::AgentState::Working);
    let changed = next_json(&lines, "p3 starting work");
    assert_eq!(
        (&changed["pane"], &changed["state"]),
        (&json!("p3"), &json!("working")),
        "{changed}"
    );
    let _ = watch.kill();
    let _ = watch.wait();
}

/// A wait on a pane that closes can never end, so it fails naming the pane.
fn a_wait_on_a_pane_that_closes_is_refused(here: &Here) {
    // The wait has to be following the daemon before the close, or it is refused as a pane
    // nobody holds: so the close waits for the daemon to log one more subscription.
    let subscribed = r#""request":"session.subscribe""#;
    let mut logging = here.daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    let replayed = match expect(&mut logging, session(follow), proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Followed(followed)) => followed.newest,
        other => panic!("a follow answered with {other:?}"),
    };
    let before = logging
        .logged_through(replayed, PATIENCE)
        .iter()
        .filter(|line| line.line.contains(subscribed))
        .count();
    let wait =
        here.spawned(&["pane", "wait", "--pane", "p3", "--until", "blocked", "--timeout", "60"]);
    logging.logged_times_until(subscribed, before + 1, PATIENCE);
    let mut control = here.daemon.connect();
    expect(&mut control, close_request("p3"), proto::Outcome::Done);
    let ended = wait.wait_with_output().unwrap();
    let refused = refused_with(&ended, 1);
    assert!(refused.contains("pane p3 closed before it was blocked"), "{refused}");
}

/// A line over 1024 bytes into `cat`, which reads in canonical mode, is discarded whole, and a
/// confirmed send says so with the window's own words rather than exiting 0.
fn a_send_the_pane_never_shows_is_refused(here: &Here) {
    ok(&here.muster(&["pane", "send", "--pane", "p1", "--enter", "cat"]));
    let mut control = here.daemon.connect();
    until_text(&mut control, "p1", "cat\n");
    let long = format!("{}end", "x".repeat(1100));
    let ran = here.muster(&["pane", "send", "--pane", "p1", "--enter", "--confirm", &long]);
    let refused = refused_with(&ran, 1);
    assert!(refused.contains("is not on it") && refused.contains("canonical mode"), "{refused}");
}

#[test]
fn only_a_missing_window_is_answered_by_the_daemon() {
    let here = Here::new(Daemon::start_built());
    let mut control = here.daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));

    let refused = refused_with(&here.muster(&["pane", "new"]), 3);
    assert!(
        refused.contains("pane send") && refused.contains("still work"),
        "a verb only a window can carry out says what works without one:\n{refused}"
    );

    let named = here.muster(&["--socket", &here.window, "pane", "read", "--pane", "p1"]);
    let refused = refused_with(&named, 3);
    assert!(
        refused.contains("--socket said"),
        "a window the caller named is not passed over for the daemon:\n{refused}"
    );

    let refused = refused_with(&here.muster(&["--no-window", "tab", "new"]), 1);
    assert!(refused.contains("needs a window"), "{refused}");
}

/// With a window open, `--no-window` answers from the daemon what the window answers.
#[test]
fn the_daemon_and_the_window_say_the_same_about_a_pane() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_detecting();
    let socket = daemon.root().join("command.sock").to_string_lossy().into_owned();
    accepted(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        command_socket_path: socket.clone(),
        ..Startup::default()
    })));
    accepted(&dispatch(request::Payload::OpenWindow(OpenWindow::default())));
    let mut here = Here::new(daemon);
    here.window = socket;

    let pane = until_some("the window to describe the pane the daemon holds", || {
        let window: Value =
            serde_json::from_slice(&here.muster(&["--json", "window"]).stdout).ok()?;
        Some(window["panes"].get(0)?["pane"].as_str()?.to_string())
    });
    let mut control = here.daemon.connect();
    until_text(&mut control, &pane, "$");
    ok(&here.muster(&["pane", "send", "--pane", &pane, "--enter", "--confirm", "echo both-ways"]));
    until_text(&mut control, &pane, "both-ways\n");

    for asked in [
        &["--json", "pane", "read", "--pane", &pane][..],
        &["--json", "pane", "read", "--pane", &pane, "--rows", "2"],
    ] {
        let through_the_window = ok(&here.muster(asked));
        let from_the_daemon = ok(&here.muster(&[&["--no-window"][..], asked].concat()));
        assert_eq!(through_the_window, from_the_daemon, "{asked:?}");
    }

    let windowed: Value = serde_json::from_str(&ok(&here.muster(&["--json", "window"]))).unwrap();
    let windowless: Value =
        serde_json::from_str(&ok(&here.muster(&["--no-window", "--json", "window"]))).unwrap();
    let described = |window: &Value| -> Vec<(Value, Value, Value)> {
        let mut panes: Vec<_> = window["panes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pane| (pane["pane"].clone(), pane["tab"].clone(), pane["state"].clone()))
            .collect();
        panes.sort_by_key(|pane| pane.0.to_string());
        panes
    };
    assert_eq!(described(&windowed), described(&windowless), "{windowed}\n{windowless}");
    assert!(windowed.get("answered_by").is_none(), "a window's answer never says it: {windowed}");
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
    let line = lines
        .recv_timeout(PATIENCE)
        .unwrap_or_else(|_| panic!("timed out waiting for {what}: nothing reached the pipe"));
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("a watch line under --json is not JSON ({error}): {line:?}"))
}
