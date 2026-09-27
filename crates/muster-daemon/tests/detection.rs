//! Agent detection in the daemon (MIP-3 section 8): which agent each pane runs and what state
//! it is in, published on the pane's record.
//!
//! The first test is the herdr probe's `detection` scenario (`tools/herdr-probe`, recorded in
//! `corpus/herdr-0.8.0/detection/`), run against this daemon: the same fake agent, the same
//! override manifest, the same four states with and without a viewer. It is the check that
//! moving detection out of herdr kept what herdr did.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::stream_message::Message as Streamed;
use support::*;

const FAKE_AGENT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/herdr-probe/fake-agent/screen-agent");
const PROBE_MANIFEST: &str = include_str!("../../../tools/herdr-probe/fake-agent/claude.toml");

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
    let claude = include_str!("../../muster-detect/manifests/claude.toml");
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
