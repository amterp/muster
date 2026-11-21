use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::persistence::record;
use crate::support::*;
use proto::AgentState::{Blocked, Idle, Working};
use serde_json::Value;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/agy");
const HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/agy/hooks.json"));
const REPORTS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/agy-1.5.14/reports.jsonl"));

struct AntigravityPane {
    daemon: Daemon,
    control: Control,
    work: PathBuf,
    sequence: usize,
}

impl AntigravityPane {
    fn start() -> Self {
        let daemon = daemon();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        until_text(&mut control, "p1", "$");
        let work = daemon.root().join("scratch cwd");
        std::fs::create_dir(&work).unwrap();
        let agent = daemon.root().join("agy");
        std::fs::write(&agent, include_str!("agy-peer.sh")).unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        daemon.type_into("p1", &format!("{} '{}' '{PLUGIN}'", agent.display(), work.display()));
        until_text(&mut control, "p1", "AGY-READY");
        let mut pane = Self { daemon, control, work, sequence: 0 };
        pane.until_state(Idle);
        pane
    }

    fn send(&mut self, action: &str, payload: &Value) -> String {
        let mut payload = payload.clone();
        payload["cwd"] = self.work.to_str().unwrap().into();
        let input = format!("{payload}\n\n");
        std::fs::write(self.work.join("input"), &input).unwrap();
        self.sequence += 1;
        self.daemon.type_into("p1", &action.replace("{number}", &self.sequence.to_string()));
        until_text(&mut self.control, "p1", &format!("AGY-DONE {}", self.sequence));
        input
    }

    fn hook(&mut self, payload: &Value, conversation: &str) {
        let hooks: Value = serde_json::from_str(HOOKS).unwrap();
        let event = payload["hookEventName"].as_str().unwrap();
        let command = hooks["muster"][event][0]["command"].as_str().unwrap();
        self.send(&format!("hook {{number}} {conversation} {command}"), payload);
        assert_eq!(written(&self.work.join("hook-output")), "{}\n");
    }

    fn status(&mut self, payload: &Value) {
        let input = self.send("status {number}", payload);
        assert_eq!(written(&self.work.join("drawn")), input);
        assert_eq!(written(&self.work.join("draw-exit")), "7\n");
    }

    fn pane(&mut self) -> proto::Pane {
        record(&snapshot(&mut self.control), "p1").clone()
    }

    fn until_state(&mut self, expected: proto::AgentState) {
        until(
            &format!("agy to be {expected:?}"),
            || {
                let pane = self.pane();
                pane.agent.as_deref() == Some("agy") && pane.agent_state() == expected
            },
            (),
        );
    }
}

fn payload(name: &str) -> Value {
    REPORTS
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("no recording for {name}"))["payload"]
        .clone()
}

fn conversation(payload: &Value) -> &str {
    payload["conversationId"].as_str().unwrap()
}

#[test]
fn recorded_antigravity_hooks_and_status_reach_the_panes_daemon() {
    let mut agent = AntigravityPane::start();
    agent.status(&payload("authenticating"));
    assert!(agent.pane().facts.is_none());
    agent.send("status-sample {number}", &payload("startup"));
    assert_eq!(agent.pane().session_id, None);

    let start = payload("start");
    let first = conversation(&start);
    agent.hook(&start, "an-unrelated-conversation");
    assert_eq!(agent.pane().session_id, None);
    agent.hook(&start, first);
    let pane = agent.pane();
    assert_eq!(pane.session_id.as_deref(), Some(first));
    assert_eq!(pane.resume.unwrap().command, ["agy", "--conversation", first]);

    agent.hook(&payload("pre-invocation"), first);
    agent.until_state(Working);

    let mut continuing = payload("stop");
    continuing["continuationActive"] = true.into();
    agent.hook(&continuing, first);
    assert_eq!(agent.pane().agent_state(), Working);

    agent.status(&payload("approval"));
    agent.until_state(Blocked);
    agent.hook(&payload("stop"), first);
    agent.until_state(Idle);

    let mut late_status = payload("approval");
    late_status["tool_confirmation_pending"] = false.into();
    late_status["context_window"]["used_percentage"] = 4.into();
    agent.send("status-sample {number}", &late_status);
    let report = written(&agent.work.join("last-report"));
    assert!(report.contains("--context-used 4"), "the status facts landed: {report}");
    assert!(!report.contains("--state"), "a late status must respect Stop: {report}");

    let renamed = payload("renamed");
    agent.status(&renamed);
    until(
        "the adapter's current title",
        || agent.pane().label.as_deref() == Some("Muster Capture"),
        (),
    );
    let facts = agent.pane().facts.unwrap();
    assert_eq!(facts.model.as_deref(), Some("Gemini 3.8 Flash (Low)"));
    assert_eq!(facts.effort.as_deref(), Some("low"));
    assert!(facts.context_used.is_some_and(|used| used > 0.0));
    assert_eq!(facts.cost_usd, None);

    let second = payload("second-start");
    agent.hook(&second, conversation(&second));
    agent.send("status-sample {number}", &renamed);
    let pane = agent.pane();
    assert_eq!(pane.session_id.as_deref(), Some(conversation(&second)));
    assert_eq!(pane.facts.and_then(|facts| facts.context_used), None);
}
