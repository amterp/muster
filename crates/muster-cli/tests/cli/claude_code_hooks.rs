//! What `extras/claude-code/hooks/hooks.json` tells the daemon, run as Claude Code runs it: each
//! command through a shell, the hook's input on stdin, `$MUSTER_DAEMON` naming a program that
//! writes down what it was asked.
//!
//! The inputs are the ones Claude Code 2.1.318 handed its hooks in
//! `corpus/claude-code-2.1.318/background-hooks.jsonl`, so a change to what a sub-agent's hook
//! says about its parent is judged against a recording, not against our idea of the payload.

use std::fs::Permissions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const RECORDED_HOOKS: &str =
    include_str!("../../../../corpus/claude-code-2.1.318/background-hooks.jsonl");

fn hooks() -> Value {
    serde_json::from_str(include_str!("../../../../extras/claude-code/hooks/hooks.json")).unwrap()
}

fn command(event: &str) -> String {
    let hooks = hooks();
    let groups = hooks["hooks"][event].as_array().unwrap_or_else(|| panic!("{event} is hooked"));
    assert_eq!(groups.len(), 1, "{event} has one group");
    groups[0]["hooks"][0]["command"].as_str().unwrap().to_string()
}

/// Every recorded input for `event`, as Claude Code sent it: the recording's own `at` and
/// `probe` labels removed.
fn recorded(event: &str) -> Vec<Value> {
    RECORDED_HOOKS
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["hook_event_name"] == event)
        .map(|mut row| {
            let object = row.as_object_mut().unwrap();
            object.remove("at");
            object.remove("probe");
            row
        })
        .collect()
}

fn is_a_subagents(payload: &Value) -> bool {
    payload.get("agent_id").is_some()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("muster-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let program = directory.join("daemon");
        std::fs::write(&program, "#!/bin/sh\necho \"$*\" >> \"$0.asked\"\n").unwrap();
        std::fs::set_permissions(&program, Permissions::from_mode(0o755)).unwrap();
        Self(directory)
    }

    fn program(&self) -> PathBuf {
        self.0.join("daemon")
    }

    /// What the daemon program was asked since the last call, one entry per run.
    fn asked(&self) -> Vec<String> {
        let path = self.0.join("daemon.asked");
        let asked = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        asked.lines().map(str::to_string).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(shell: &str, command: &str, daemon: Option<&Path>, payload: &Value) {
    let mut hook = Command::new(shell);
    hook.args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(daemon) = daemon {
        hook.env("MUSTER_DAEMON", daemon);
    }
    let mut hook = hook.spawn().unwrap();
    hook.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let output = hook.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{shell}: a hook never fails the turn");
}

fn shells() -> impl Iterator<Item = &'static str> {
    ["/bin/sh", "/bin/zsh"].into_iter().filter(|shell| Path::new(shell).exists())
}

/// A sub-agent's tool calls run the same hooks as the session's own, with an `agent_id`
/// added, and arrive while the parent has ended its turn and shows `Waiting for 1 background
/// agent`. Reported as the parent working, they hold its pane at working over a screen that
/// says it takes input.
#[test]
fn a_subagents_tool_call_says_nothing_about_its_parent() {
    let scratch = Scratch::new("subagent");
    for event in ["PostToolUse", "PostToolUseFailure"] {
        let command = command(event);
        // The recording holds no failures; a failed call carries the same fields as a finished
        // one, so its inputs are the recorded ones renamed.
        let payloads: Vec<Value> = recorded("PostToolUse")
            .into_iter()
            .map(|mut payload| {
                payload["hook_event_name"] = json!(event);
                payload
            })
            .collect();
        assert!(payloads.iter().any(is_a_subagents), "the recording has a sub-agent's call");
        assert!(payloads.iter().any(|payload| !is_a_subagents(payload)), "and the parent's own");
        for shell in shells() {
            for payload in &payloads {
                run(shell, &command, Some(&scratch.program()), payload);
                let expected: &[&str] = if is_a_subagents(payload) {
                    &[]
                } else {
                    &["report --agent claude --state working"]
                };
                assert_eq!(scratch.asked(), expected, "{shell}, {event}, {payload}");
            }
        }
    }
}

/// Text that happens to say `agent_id` inside a tool's output is escaped in the JSON, so it
/// does not read as the field.
#[test]
fn a_parents_tool_output_mentioning_agent_id_still_reports() {
    let scratch = Scratch::new("quoted");
    let payload = json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_response": { "stdout": "{\"agent_id\": \"a9e7b710705d7b985\"}" },
    });
    for shell in shells() {
        run(shell, &command("PostToolUse"), Some(&scratch.program()), &payload);
        assert_eq!(scratch.asked(), ["report --agent claude --state working"], "{shell}");
    }
}

/// A sub-agent asking permission is still a dialog somebody has to answer.
#[test]
fn a_subagents_permission_request_still_reports_blocked() {
    let scratch = Scratch::new("permission");
    let payload = json!({
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "agent_id": "a9e7b710705d7b985",
        "agent_type": "general-purpose",
    });
    for shell in shells() {
        run(shell, &command("PermissionRequest"), Some(&scratch.program()), &payload);
        assert_eq!(scratch.asked(), ["report --agent claude --state blocked"], "{shell}");
    }
}

#[test]
fn the_working_hooks_do_nothing_outside_a_muster_pane() {
    let payload = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"});
    for shell in shells() {
        for event in ["PostToolUse", "PostToolUseFailure"] {
            run(shell, &command(event), None, &payload);
        }
    }
}
