//! Agent detection in the daemon (MIP-3 section 8): which agent each pane runs and what state
//! it is in, published on the pane's record.
//!
//! The first test is the herdr probe's `detection` scenario (`tools/herdr-probe`, recorded in
//! `corpus/herdr-0.8.0/detection/`), run against this daemon: the same fake agent, the same
//! override manifest, the same four states with and without a viewer. It is the check that
//! moving detection out of herdr kept what herdr did.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::stream_message::Message as Streamed;

const FAKE_AGENT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/herdr-probe/fake-agent/screen-agent");
const PROBE_MANIFEST: &str = include_str!("../../../../tools/herdr-probe/fake-agent/claude.toml");

/// How long herdr v0.8.0 took to settle each state in the probe's run
/// (`corpus/herdr-0.8.0/detection/FACTS.json`), with no viewer and with one.
const HERDR_UNVIEWED: [f64; 4] = [2.09, 0.78, 0.26, 0.26];
const HERDR_VIEWED: [f64; 4] = [0.26, 0.52, 0.26, 0.26];

/// A settle this slow is a detection that is broken, not a slow one. The first settle of a run
/// includes the three seconds of grace a newly recognised agent gets, as herdr's did.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// A muster home of the test's own, holding detection overrides and the fake agent under
/// whatever names it is to answer to. Gone when the test is.
struct Home(PathBuf);

impl Home {
    fn new(test: &str, overrides: &[(&str, &str)], agents: &[&str]) -> Home {
        let home =
            std::env::temp_dir().join(format!("muster-detection-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let detection = home.join("agent-detection");
        std::fs::create_dir_all(&detection).unwrap();
        for (file, text) in overrides {
            std::fs::write(detection.join(file), text).unwrap();
        }
        std::fs::create_dir_all(home.join("bin")).unwrap();
        for agent in agents {
            let path = home.join("bin").join(agent);
            std::fs::copy(FAKE_AGENT, &path).unwrap();
        }
        Home(home)
    }

    fn agent(&self, name: &str) -> PathBuf {
        self.0.join("bin").join(name)
    }

    fn overrides(&self) -> PathBuf {
        self.0.join("agent-detection")
    }

    fn daemon(&self) -> Daemon {
        daemon_with(&[("MUSTER_HOME", self.0.to_str().unwrap())])
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A pane at its shell, running the fake agent as a job the shell started.
fn run_agent(control: &mut Control, input: &mut Input, pane: &str, agent: &Path) {
    let create = proto::pane_request::Create {
        grid: Some(proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 }),
        ..create(pane, in_new_tab(&format!("t-{pane}")))
    };
    make(control, create);
    until_text(control, pane, "$");
    type_line(input, pane, &agent.display().to_string());
}

fn type_line(input: &mut Input, pane: &str, text: &str) {
    input.send(pane, Event::Send(input_event::Send { text: text.to_string(), enter: true }));
}

/// The pane's agent and state, as the daemon publishes them.
fn detected(control: &mut Control, pane: &str) -> (Option<String>, proto::AgentState) {
    let record = snapshot(control)
        .panes
        .into_iter()
        .find(|record| record.pane == pane)
        .unwrap_or_else(|| panic!("no pane {pane}"));
    let state = record.agent_state();
    (record.agent, state)
}

fn until_detected(
    control: &mut Control,
    pane: &str,
    agent: Option<&str>,
    state: proto::AgentState,
) {
    until_some(&format!("{pane} to be {agent:?} {state:?}"), || {
        (detected(control, pane) == (agent.map(str::to_string), state)).then_some(())
    });
}

/// Types a command to the fake agent and times how long the daemon takes to agree.
fn settle(
    control: &mut Control,
    input: &mut Input,
    command: &str,
    want: proto::AgentState,
) -> Duration {
    type_line(input, "p1", command);
    let started = Instant::now();
    while started.elapsed() < SETTLE_LIMIT {
        if detected(control, "p1").1 == want {
            return started.elapsed();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{command} did not settle as {want:?} within {SETTLE_LIMIT:?}");
}

const CYCLE: [(&str, proto::AgentState); 4] = [
    ("working", proto::AgentState::Working),
    ("idle", proto::AgentState::Idle),
    ("blocked", proto::AgentState::Blocked),
    ("idle", proto::AgentState::Idle),
];

/// A bridge drawing the pane, reading and crediting everything, until it is dropped.
struct Viewer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Viewer {
    fn attach(daemon: &Daemon, pane: &str) -> Viewer {
        let mut stream = attached(daemon, pane, false);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match stream.next_within(Duration::from_millis(50)) {
                    Some(Some(Streamed::Output(bytes))) => stream.credit(bytes.len() as u64),
                    Some(None) => return,
                    _ => {}
                }
            }
        });
        Viewer { stop, thread: Some(thread) }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn the_herdr_probe_detection_scenario_settles_with_and_without_a_viewer() {
    let home = Home::new("probe", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));

    // Recognised by its name, and read by the override: its idle marker is the override's.
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);

    let unviewed: Vec<Duration> = CYCLE
        .iter()
        .map(|&(command, want)| settle(&mut control, &mut input, command, want))
        .collect();
    let bridge = Viewer::attach(&daemon, "p1");
    std::thread::sleep(Duration::from_secs(1));
    let viewed: Vec<Duration> = CYCLE
        .iter()
        .map(|&(command, want)| settle(&mut control, &mut input, command, want))
        .collect();
    drop(bridge);

    let seconds = |settled: &[Duration]| -> Vec<String> {
        settled.iter().map(|elapsed| format!("{:.2}", elapsed.as_secs_f64())).collect()
    };
    println!("settled with no viewer: {:?} (herdr: {HERDR_UNVIEWED:?})", seconds(&unviewed));
    println!("settled with a viewer:  {:?} (herdr: {HERDR_VIEWED:?})", seconds(&viewed));
}

const SPRITE: &str = r#"
id = "sprite"

[[rules]]
id = "working"
state = "working"
priority = 10
region = "whole_recent"
contains = ["probe-state:working"]
"#;

#[test]
fn an_agent_whose_manifest_is_deleted_is_published_as_no_agent() {
    let home = Home::new("deleted", &[("sprite.toml", SPRITE)], &["sprite"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("sprite"));
    until_detected(&mut control, "p1", Some("sprite"), proto::AgentState::Idle);
    settle(&mut control, &mut input, "working", proto::AgentState::Working);

    std::fs::remove_file(home.overrides().join("sprite.toml")).unwrap();
    let send = session(proto::session_request::Request::SendManifests(proto::SendManifests {
        engine: 0,
        manifests: Vec::new(),
    }));
    expect(&mut control, send, proto::Outcome::Done);
    until_detected(&mut control, "p1", None, proto::AgentState::Unknown);
}

/// Loading manifests reads the override directory, which can sit on a mount that hangs. A FIFO
/// stands in for one: reading it waits until somebody opens its other end.
#[test]
fn a_manifest_load_that_hangs_holds_up_no_other_request() {
    let home = Home::new("hung", &[], &[]);
    let daemon = home.daemon();
    until(
        "the daemon's first manifest load",
        || written(&daemon.root().join("daemon.log")).contains("daemon.detect.loaded"),
        (),
    );
    let fifo = home.overrides().join("claude.toml");
    let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo reads a NUL-terminated path the test owns.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

    let mut sending = daemon.connect();
    sending.send(session(proto::session_request::Request::SendManifests(
        proto::SendManifests::default(),
    )));
    std::thread::sleep(Duration::from_millis(200));
    let (answered, answer) = std::sync::mpsc::channel();
    let socket = daemon.socket_path().to_path_buf();
    std::thread::spawn(move || {
        let mut control = Control::connect(&socket);
        let _ = answered.send(snapshot(&mut control));
    });
    let took = answer.recv_timeout(Duration::from_secs(1));
    // Lets the load finish, whatever the verdict.
    drop(std::fs::OpenOptions::new().write(true).open(&fifo));
    assert!(took.is_ok(), "a snapshot waited on a manifest load reading a hung file");
}

#[test]
fn manifests_sent_at_connect_leave_an_unchanged_agents_state_alone() {
    let home = Home::new("connect", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);
    settle(&mut control, &mut input, "working", proto::AgentState::Working);

    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    // What an app sends when it connects: its claude, which the override shadows, and a newer
    // codex, which changes only codex.
    let codex = "id = \"codex\"\nversion = \"9999.1.1.1\"\nmin_engine_version = 1\n\n\
                 [[rules]]\nid = \"busy\"\nstate = \"working\"\ncontains = [\"thinking\"]\n";
    let claude = include_str!("../../../muster-detect/manifests/claude.toml");
    let manifest = |agent: &str, toml: &str| proto::Manifest {
        agent: agent.to_string(),
        toml: toml.to_string(),
    };
    let send = session(proto::session_request::Request::SendManifests(proto::SendManifests {
        engine: 4,
        manifests: vec![manifest("claude", claude), manifest("codex", codex)],
    }));
    assert_eq!(control.ask(send).outcome(), proto::Outcome::Done);

    let deadline = Instant::now() + Duration::from_secs(2);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Some(proto::control_message::Message::Event(event)) = control.next_message(left) else {
            continue;
        };
        if let Some(proto::event::Event::PaneChanged(changed)) = event.event {
            let record = changed.pane.expect("a changed pane's record");
            assert_eq!(
                record.agent_state(),
                proto::AgentState::Working,
                "the working agent flapped when the app sent manifests"
            );
        }
    }
    assert_eq!(
        detected(&mut control, "p1"),
        (Some("claude".to_string()), proto::AgentState::Working)
    );
}

#[test]
fn what_an_agent_reported_about_itself_stays_while_it_runs_and_goes_with_it() {
    let home = Home::new("facts", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");

    // A statusline can report before detection has recognised the agent it belongs to.
    let model = proto::pane_request::Report {
        pane: "p1".to_string(),
        model: Some("Opus".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(proto::pane_request::Request::Report(model)), proto::Outcome::Done);
    type_line(&mut input, "p1", &home.agent("claude").display().to_string());
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);
    let facts = |control: &mut Control| {
        snapshot(control).panes.into_iter().find(|record| record.pane == "p1").and_then(|p| p.facts)
    };
    assert_eq!(facts(&mut control).and_then(|facts| facts.model).as_deref(), Some("Opus"));

    type_line(&mut input, "p1", "quit");
    until_detected(&mut control, "p1", None, proto::AgentState::Unknown);
    assert_eq!(facts(&mut control), None, "the agent's facts left with it");
}

/// A working agent stays working across a handoff: the new daemon goes on from where the old
/// one's detection was, rather than finding the agent anew and passing it through the grace a
/// newly recognized agent gets, as idle.
#[test]
fn a_working_agent_stays_working_across_a_handoff() {
    let home = Home::new("handoff", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let mut daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);
    settle(&mut control, &mut input, "working", proto::AgentState::Working);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let mut control = daemon.connect();
    let subscribed = control.ask(subscribe_request());
    let Some(proto::answer::Detail::Snapshot(snapshot)) = subscribed.answer.detail else {
        panic!("no snapshot in {:?}", subscribed.answer)
    };
    let record = snapshot.panes.iter().find(|record| record.pane == "p1").unwrap();
    assert_eq!(record.agent_state(), proto::AgentState::Working, "handed over as working");
    // Past the three seconds a newly recognized agent is held idle for.
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut published = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some(proto::control_message::Message::Event(event)) = control.next_message(left)
            && let Some(proto::event::Event::PaneChanged(changed)) = event.event
            && let Some(record) = changed.pane.filter(|record| record.pane == "p1")
        {
            published.push(record.agent_state());
        }
    }
    assert!(
        published.iter().all(|state| *state == proto::AgentState::Working),
        "published after the handoff: {published:?}"
    );
    assert_eq!(
        detected(&mut control, "p1"),
        (Some("claude".to_string()), proto::AgentState::Working)
    );
}

fn finished_unseen(control: &mut Control, pane: &str) -> bool {
    snapshot(control).panes.into_iter().find(|record| record.pane == pane).unwrap().finished_unseen
}

fn seen(control: &mut Control, panes: &[&str], outcome: proto::Outcome) {
    let seen =
        proto::pane_request::Seen { panes: panes.iter().map(|pane| (*pane).to_string()).collect() };
    expect(control, pane(proto::pane_request::Request::Seen(seen)), outcome);
}

/// An agent that stops working with nobody looking has finished something unseen. The daemon
/// keeps that until somebody sees the pane, so it outlives the window that was not looking, and
/// every client connecting later hears it.
#[test]
fn a_finish_nobody_has_seen_is_kept_until_somebody_sees_it() {
    let home = Home::new("unseen", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);
    assert!(!finished_unseen(&mut control, "p1"), "an agent appearing has finished nothing");

    settle(&mut control, &mut input, "working", proto::AgentState::Working);
    settle(&mut control, &mut input, "idle", proto::AgentState::Idle);
    assert!(finished_unseen(&mut control, "p1"));
    drop(control);
    let mut control = daemon.connect();
    assert!(finished_unseen(&mut control, "p1"), "a client connecting later hears it");

    seen(&mut control, &["p1"], proto::Outcome::Done);
    assert!(!finished_unseen(&mut control, "p1"));
    seen(&mut control, &["p1"], proto::Outcome::AlreadySo);
    seen(&mut control, &["p1", "p9"], proto::Outcome::AlreadySo);
    seen(&mut control, &["p9"], proto::Outcome::NotThere);

    settle(&mut control, &mut input, "blocked", proto::AgentState::Blocked);
    settle(&mut control, &mut input, "idle", proto::AgentState::Idle);
    assert!(finished_unseen(&mut control, "p1"), "a prompt answered, then idle");
    settle(&mut control, &mut input, "working", proto::AgentState::Working);
    assert!(!finished_unseen(&mut control, "p1"), "working again is not done");

    type_line(&mut input, "p1", "quit");
    until_detected(&mut control, "p1", None, proto::AgentState::Unknown);
    assert!(finished_unseen(&mut control, "p1"), "it ended while working");

    // A window showing a pane that closed as it asked still has the others seen.
    seen(&mut control, &["p1", "p9"], proto::Outcome::Done);
    assert!(!finished_unseen(&mut control, "p1"), "seen alongside a pane that is gone");
}

/// What nobody has seen yet is still unseen after a handoff, since the pane's record goes over
/// whole.
#[test]
fn a_finish_nobody_has_seen_is_still_unseen_after_a_handoff() {
    let home = Home::new("unseen-handoff", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let mut daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);
    settle(&mut control, &mut input, "working", proto::AgentState::Working);
    settle(&mut control, &mut input, "idle", proto::AgentState::Idle);
    assert!(finished_unseen(&mut control, "p1"));

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    assert!(finished_unseen(&mut daemon.connect(), "p1"));
}

fn report_state(control: &mut Control, agent: &str, state: proto::AgentState) -> proto::Answer {
    let mut report = proto::pane_request::Report {
        pane: "p1".to_string(),
        agent: agent.to_string(),
        ..Default::default()
    };
    report.set_state(state);
    control.ask(pane(proto::pane_request::Request::Report(report))).answer
}

fn state_reported(control: &mut Control) -> bool {
    snapshot(control).panes.into_iter().find(|record| record.pane == "p1").unwrap().state_reported
}

/// A report goes with the pane: after a handoff the new daemon holds the agent at what it said,
/// not at what the rules read, until something ends the report.
#[test]
fn an_agents_own_report_survives_a_handoff() {
    use proto::AgentState::{Blocked, Idle};
    let home = Home::new("reported-handoff", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let mut daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), Idle);
    assert_eq!(report_state(&mut control, "claude", Blocked).outcome(), proto::Outcome::Done);
    until_detected(&mut control, "p1", Some("claude"), Blocked);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let mut control = daemon.connect();
    assert_eq!(detected(&mut control, "p1"), (Some("claude".to_string()), Blocked));
    assert!(state_reported(&mut control), "the successor says whose word it is");
}

/// An agent's own word on its state outranks what its screen reads while it is fresh. A
/// working report the screen stops moving under goes stale - an agent interrupted mid-turn
/// says nothing - and the rules take over again.
#[test]
fn an_agents_own_report_wins_until_its_screen_stops_moving() {
    let home = Home::new("reported", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Idle);

    let answer = report_state(&mut control, "claude", proto::AgentState::Working);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    until_detected(&mut control, "p1", Some("claude"), proto::AgentState::Working);
    assert!(state_reported(&mut control));

    // Nothing is drawn from here on, so the report goes stale after ten quiet seconds.
    muster_harness::until_within(
        "the report to go stale",
        Duration::from_secs(15),
        || detected(&mut control, "p1").1 == proto::AgentState::Idle,
        (),
    );
    assert!(!state_reported(&mut control));

    let refused = report_state(&mut control, "", proto::AgentState::Idle);
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "a state needs its agent");
    let refused = report_state(&mut control, "claude", proto::AgentState::Unknown);
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "unknown is not reported");
}

const CLAUDE_CODE_HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code/hooks/hooks.json"));

/// The one command `extras/claude-code/hooks/hooks.json` runs for `event`, and its matcher.
fn claude_code_hook(event: &str) -> (Option<String>, String) {
    let hooks: serde_json::Value = serde_json::from_str(CLAUDE_CODE_HOOKS).unwrap();
    let groups = hooks["hooks"][event].as_array().unwrap_or_else(|| panic!("no {event} hook"));
    assert_eq!(groups.len(), 1, "one {event} entry");
    let matcher = groups[0]["matcher"].as_str().map(str::to_string);
    let commands = groups[0]["hooks"].as_array().unwrap();
    assert_eq!(commands.len(), 1, "one {event} command");
    (matcher, commands[0]["command"].as_str().unwrap().to_string())
}

/// Claude Code's hooks, run as Claude Code runs a command hook - by `sh -c`, in the pane's
/// environment - report the state each event means. The events and matchers are Claude Code's
/// documented ones: a prompt submitted and a tool finished mean working, a permission prompt or
/// a question waiting on you means blocked, and a turn that ended, well or not, means idle.
#[test]
fn claude_codes_hooks_report_working_blocked_and_idle() {
    use proto::AgentState::{Blocked, Idle, Working};
    let home = Home::new("hooks", &[("claude.toml", PROBE_MANIFEST)], &["claude"]);
    let daemon = home.daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    run_agent(&mut control, &mut input, "p1", &home.agent("claude"));
    until_detected(&mut control, "p1", Some("claude"), Idle);

    let events = [
        ("UserPromptSubmit", None, Working),
        ("PermissionRequest", None, Blocked),
        ("PostToolUse", None, Working),
        ("PermissionRequest", None, Blocked),
        ("PostToolUseFailure", None, Working),
        ("Notification", Some("permission_prompt|elicitation_dialog"), Blocked),
        ("Stop", None, Idle),
        ("UserPromptSubmit", None, Working),
        ("StopFailure", None, Idle),
    ];
    for (event, matcher, state) in events {
        let (matched, command) = claude_code_hook(event);
        assert_eq!(matched.as_deref(), matcher, "{event}'s matcher");
        let ran = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .env("MUSTER_DAEMON", env!("CARGO_BIN_EXE_muster-daemon"))
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .env("MUSTER_PANE", "p1")
            .status()
            .unwrap();
        assert!(ran.success(), "{event}'s hook");
        until_detected(&mut control, "p1", Some("claude"), state);
        assert!(state_reported(&mut control), "{event} is the agent's own word");
    }
}
