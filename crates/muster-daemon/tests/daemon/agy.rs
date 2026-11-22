use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::api_panes::current_pane;
use crate::attention::msg;
use crate::harness_hooks::legacy_report;
use crate::persistence::{directory, record, until_saved};
use crate::resume::restarted;
use crate::support::*;
use proto::AgentState::{Blocked, Idle, Working};
use proto::msg_request::{self, Request as Asked};
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
        let work = directory(&daemon, "scratch cwd");
        let home = daemon.root().join("home");
        let bin = home.join(".muster/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let agent = bin.join("agy");
        std::fs::write(&agent, include_str!("agy-peer.sh")).unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(work.join("plugin"), PLUGIN).unwrap();
        std::fs::write(
            home.join(".profile"),
            "PATH=\"$HOME/.muster/bin:$PATH\"\nexport PATH\nPS1='$ '\n",
        )
        .unwrap();
        let create = proto::pane_request::Create {
            cwd: Some(work.display().to_string()),
            ..create("p1", in_new_tab("t1"))
        };
        make(&mut control, create);
        let mut pane = Self { daemon, control, work, sequence: 0 };
        pane.launch();
        pane
    }

    fn launch(&mut self) {
        until_text(&mut self.control, "p1", "$");
        self.daemon.type_into("p1", "agy");
        until_text(&mut self.control, "p1", "AGY-READY");
        self.until_state(Idle);
    }

    fn send(&mut self, action: &str, payload: &Value) -> String {
        let mut payload = payload.clone();
        if payload["cwd"] == "/tmp/capture" {
            payload["cwd"] = self.work.to_str().unwrap().into();
        }
        if let Some(path) = payload["transcriptPath"].as_str()
            && let Some(relative) = path.strip_prefix("/tmp/capture/")
        {
            payload["transcriptPath"] = self.work.join(relative).display().to_string().into();
        }
        let input = format!("{payload}\n\n");
        std::fs::write(self.work.join("input"), &input).unwrap();
        self.command(action);
        input
    }

    fn command(&mut self, action: &str) {
        self.sequence += 1;
        self.daemon.type_into("p1", &action.replace("{number}", &self.sequence.to_string()));
        until_text(&mut self.control, "p1", &format!("AGY-DONE {}", self.sequence));
    }

    fn hook(&mut self, payload: &Value, conversation: &str) {
        self.hook_as("hook", payload, conversation);
    }

    fn hook_as(&mut self, action: &str, payload: &Value, conversation: &str) {
        let hooks: Value = serde_json::from_str(HOOKS).unwrap();
        let event = payload["hookEventName"].as_str().unwrap();
        let command = hooks["muster"][event][0]["command"].as_str().unwrap();
        self.send(&format!("{action} {{number}} {conversation} {command}"), payload);
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

    fn transcript(&self) -> Option<api::SessionTranscript> {
        current_pane(&self.daemon, "p1").session_transcript
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
    let resume = pane.resume.unwrap().command;
    assert_eq!(&resume[1..], ["--conversation", first]);
    assert_eq!(PathBuf::from(&resume[0]).file_name().unwrap(), "agy");
    let transcript = agent.transcript().expect("the hook reported the transcript");
    assert_eq!(transcript.session_id, first);
    assert_eq!(transcript.directory, agent.work.display().to_string());
    assert_eq!(transcript.path, agent.work.join("first-full.jsonl").display().to_string());
    assert!(agent.pane().context_change.is_none());

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

    let mut second = payload("second-start");
    second.as_object_mut().unwrap().remove("transcriptPath");
    agent.hook(&second, conversation(&second));
    agent.send("status-sample {number}", &renamed);
    let pane = agent.pane();
    assert_eq!(pane.session_id.as_deref(), Some(conversation(&second)));
    assert_eq!(pane.facts.and_then(|facts| facts.context_used), None);
    let change = pane.context_change.expect("a new session in the same launch clears context");
    assert_eq!(change.kind(), proto::ContextChangeKind::Cleared);
    assert!(agent.transcript().is_none());
    agent.hook(&payload("second-start"), conversation(&second));
    assert_eq!(agent.pane().context_change, Some(change));
    let replaced = agent.transcript().expect("the next valid path replaces the missing one");
    assert_eq!(replaced.session_id, conversation(&second));
    assert_eq!(replaced.path, agent.work.join("second-full.jsonl").display().to_string());
}

#[test]
fn antigravity_provenance_requires_the_foreground_hook_and_the_live_directory() {
    let mut agent = AntigravityPane::start();
    let start = payload("start");
    let first = conversation(&start);
    let mut detached = start.clone();
    detached["cwd"] = agent.work.display().to_string().into();
    std::fs::write(agent.work.join("detached-input"), detached.to_string()).unwrap();
    let output = Command::new("python3")
        .args([
            "-c",
            // With no PATH, Linux's python3 leaves sys.executable empty.
            "import os, sys; exe = sys.executable or os.path.realpath('/proc/self/exe'); \
             os.setsid(); os.execv(exe, [exe, *sys.argv[1:]])",
        ])
        .arg(format!("{PLUGIN}/report.py"))
        .arg("hook")
        .env_clear()
        .env("ANTIGRAVITY_CONVERSATION_ID", first)
        .env("MUSTER_DAEMON", env!("CARGO_BIN_EXE_muster-daemon"))
        .env("MUSTER_DAEMON_SOCKET", agent.daemon.socket_path())
        .env("MUSTER_PANE", "p1")
        .env("TMPDIR", &agent.work)
        .stdin(Stdio::from(std::fs::File::open(agent.work.join("detached-input")).unwrap()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"{}\n");
    assert!(agent.pane().session_id.is_none());

    agent.hook_as("nested", &start, first);
    assert!(agent.pane().session_id.is_none());
    assert!(agent.transcript().is_none());

    let mut invalid = start.clone();
    invalid["transcriptPath"] = "relative.jsonl".into();
    agent.hook(&invalid, first);
    assert_eq!(agent.pane().session_id.as_deref(), Some(first));
    assert!(agent.transcript().is_none());

    let mut status = payload("renamed");
    status["transcript_path"] = "/desktop/wrong.jsonl".into();
    agent.send("status-sample {number}", &status);
    assert!(agent.transcript().is_none());

    let mut other_directory = start;
    other_directory["cwd"] = directory(&agent.daemon, "different cwd").display().to_string().into();
    let first = conversation(&other_directory);
    agent.hook(&other_directory, first);
    assert!(agent.transcript().is_none());

    let pre = payload("pre-invocation");
    agent.hook(&pre, conversation(&pre));
    assert_eq!(agent.transcript().unwrap().session_id, conversation(&pre));
}

#[test]
fn antigravity_resumes_a_saved_id_and_reports_fresh_provenance() {
    let mut agent = AntigravityPane::start();
    let start = payload("start");
    let first = conversation(&start);
    agent.hook(&start, first);
    let before = agent.transcript().expect("initial transcript");
    until_saved(&agent.daemon, first);

    agent.control = restarted(&mut agent.daemon, &mut agent.control);
    until_text(&mut agent.control, "p1", "AGY-READY");
    agent.until_state(Idle);
    assert_eq!(written(&agent.work.join("argv")), format!("--conversation\n{first}\n"));
    assert_eq!(agent.pane().session_id.as_deref(), Some(first));
    assert!(agent.transcript().is_none());

    agent.hook(&payload("pre-invocation"), first);
    assert_eq!(agent.transcript(), Some(before));
    assert!(agent.pane().context_change.is_none());
}

#[test]
fn antigravity_drops_the_previous_launch_and_ignores_a_stale_invocation() {
    let mut agent = AntigravityPane::start();
    let start = payload("start");
    agent.hook(&start, conversation(&start));
    agent.daemon.type_into("p1", "quit");
    until("the old agent to leave", || agent.pane().agent.is_none(), ());
    agent.launch();

    let mut next = payload("second-start");
    next["hookEventName"] = "PreInvocation".into();
    let second = conversation(&next);
    agent.hook(&next, second);
    assert_eq!(agent.transcript().unwrap().session_id, second);
    assert!(agent.pane().context_change.is_none());

    let stale = payload("pre-invocation");
    agent.hook(&stale, conversation(&stale));
    assert_eq!(agent.transcript().unwrap().session_id, second);
}

#[test]
fn antigravity_retries_a_clear_after_the_start_report_times_out() {
    let mut agent = AntigravityPane::start();
    let start = payload("start");
    let first = conversation(&start);
    agent.hook(&start, first);

    let mut next = payload("second-start");
    std::fs::write(agent.work.join("hold-report"), "hold once\n").unwrap();
    agent.hook(&next, conversation(&next));
    assert!(agent.work.join("held-report").exists());
    assert_eq!(agent.transcript().unwrap().session_id, first);
    assert!(agent.pane().context_change.is_none());

    next["hookEventName"] = "PreInvocation".into();
    let second = conversation(&next);
    agent.hook(&next, second);
    assert_eq!(agent.transcript().unwrap().session_id, second);
    let change = agent.pane().context_change.expect("retry reports the clear");
    assert_eq!(change.kind(), proto::ContextChangeKind::Cleared);

    agent.hook(&payload("pre-invocation"), first);
    assert_eq!(agent.transcript().unwrap().session_id, second);
    assert_eq!(agent.pane().context_change, Some(change));
}

#[test]
fn antigravity_keeps_session_identity_when_protocol_1_18_omits_provenance() {
    let mut agent = AntigravityPane::start();
    let socket = agent.daemon.root().join("old-report.sock");
    std::fs::write(agent.work.join("old-socket"), socket.display().to_string()).unwrap();
    let receiver = legacy_report(&socket);
    let start = payload("start");
    agent.hook_as("old-hook", &start, conversation(&start));
    assert!(written(&agent.work.join("last-report")).contains("--session-transcript"));
    let report = receiver.join().unwrap();
    assert_eq!(report.session_id.as_deref(), Some(conversation(&start)));
    assert!(report.session_transcript.is_none());
}

#[test]
fn antigravity_a_compact_shaped_stop_leaves_context_change_absent() {
    let mut agent = AntigravityPane::start();
    let start = payload("start");
    let conversation = conversation(&start);
    agent.hook(&start, conversation);
    agent.hook(&payload("pre-invocation"), conversation);
    agent.until_state(Working);
    let mut stop = payload("stop");
    stop["terminationReason"] = "TERMINAL_CUSTOM_HOOK".into();
    stop["lastUserInput"] = "/compact".into();
    agent.hook(&stop, conversation);
    agent.until_state(Idle);
    assert_eq!(agent.pane().session_id.as_deref(), Some(conversation));
    assert!(agent.pane().context_change.is_none());
}

#[test]
fn antigravity_wakes_an_empty_composer_and_holds_drafts_dialogs_and_working_turns() {
    let mut agent = AntigravityPane::start();
    let mut client = agent.daemon.api();
    let wakes = agent.work.join("wakes");
    let recipient = msg_request::Caller { pane: Some("p1".into()), ..Default::default() };
    assert!(agent.pane().session_id.is_none(), "the first brief precedes the first session");

    for (index, frame) in ["idle", "draft", "dialog", "working"].into_iter().enumerate() {
        if frame != "idle" {
            agent.command(&format!("frame {{number}} {frame}"));
            match frame {
                "dialog" => agent.until_state(Blocked),
                "working" => agent.until_state(Working),
                _ => {}
            }
        }
        let body = format!("brief while {frame}");
        let asked = client.ask(post(api::NewPost {
            operation: frame.into(),
            to: vec!["p1".into()],
            body: body.clone(),
            ..Default::default()
        }));
        assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
        let posted = asked.posted();
        if frame != "idle" {
            let outcome = &posted.recipient_outcomes[0];
            assert_eq!(outcome.disposition(), api::PostDisposition::Deferred, "{outcome:?}");
            assert!(outcome.reason.is_some());
            if frame == "draft" {
                until(
                    "the doorbell to see the draft",
                    || agent.pane().wake_held.is_some_and(|reason| reason.contains("not sent")),
                    (),
                );
            } else {
                let reason = if frame == "dialog" {
                    "it is at a dialog, waiting for somebody to answer it"
                } else {
                    "waiting for the agent to go idle"
                };
                assert_eq!(outcome.reason.as_deref(), Some(reason));
                // Activity deferrals do not set wake_held. Leave the frame up past the
                // doorbell's three-second quiet period so a stray ring can reach the peer.
                std::thread::sleep(Duration::from_secs(4));
            }
            assert_eq!(written(&wakes).lines().count(), index, "wake reached {frame}");
            agent.command("frame {number} idle");
            agent.until_state(Idle);
        }
        let heard = until_some("a typed wake", || {
            let text = std::fs::read_to_string(&wakes).ok()?;
            (text.lines().count() > index).then_some(text)
        });
        let (received_at, notice) = heard.lines().nth(index).unwrap().split_once('\t').unwrap();
        assert_eq!(received_at, "idle", "{notice}");
        assert!(notice.starts_with(&format!("[muster] {}", posted.group)), "{notice}");
        assert!(notice.contains(&format!("muster msg read --group {}", posted.group)), "{notice}");

        let read = Asked::Read(msg_request::Read { group: Some(posted.group.clone()) });
        let read = expect(&mut agent.control, msg(&recipient, read), proto::Outcome::Done);
        let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
            answer: Some(proto::msg_answer::Answer::Entries(entries)),
            ..
        })) = read.answer.detail
        else {
            panic!("the wake's read got no entries: {:?}", read.answer);
        };
        assert!(entries.groups.iter().flat_map(|group| &group.entries).any(|entry| {
            matches!(&entry.what, Some(proto::msg_answer::entry::What::Message(post)) if post.body == body)
        }));
    }
}
