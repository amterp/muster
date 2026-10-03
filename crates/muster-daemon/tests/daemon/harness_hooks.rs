//! The hooks each harness's wiring in `extras/` installs, run as the harness runs them: what each
//! event reports to the daemon owning the pane. A harness's hooks are the only way its own word
//! reaches Muster, so every event a file names is pinned here, and every event that ends a turn
//! must report idle - an agent that declared a wait is moved on only by its own idle report.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use super::claude_code::Scratch;

struct Harness {
    /// Its directory under `extras/`.
    extras: &'static str,
    hooks: &'static str,
    /// Each event its hooks answer, with the arguments `$MUSTER_DAEMON` is run with.
    reports: &'static [(&'static str, &'static str)],
    /// The events that end a turn.
    turn_ends: &'static [&'static str],
}

const CLAUDE_CODE: Harness = Harness {
    extras: "claude-code",
    hooks: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../extras/claude-code/hooks/hooks.json"
    )),
    reports: &[
        ("SessionStart", "[report][--clear]"),
        ("SubagentStart", "[report][--subagent-started]"),
        ("SubagentStop", "[report][--subagent-stopped]"),
        ("UserPromptSubmit", "[report][--agent][claude][--state][working][--waiting][]"),
        ("PostToolUse", "[report][--agent][claude][--state][working]"),
        ("PostToolUseFailure", "[report][--agent][claude][--state][working]"),
        ("PermissionRequest", "[report][--agent][claude][--state][blocked]"),
        ("Notification", "[report][--agent][claude][--state][blocked]"),
        ("Stop", "[report][--agent][claude][--state][idle]"),
        ("StopFailure", "[report][--agent][claude][--state][idle]"),
    ],
    turn_ends: &["Stop", "StopFailure"],
};

/// Codex has no `Notification`, and `Interrupt` is how a turn Esc ended says so: no `Stop`
/// follows it (docs/observations/codex-0.154.0.md).
const CODEX: Harness = Harness {
    extras: "codex",
    hooks: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../extras/codex/hooks/hooks.json"
    )),
    reports: &[
        ("SessionStart", "[report][--clear]"),
        ("SubagentStart", "[report][--subagent-started]"),
        ("SubagentStop", "[report][--subagent-stopped]"),
        ("UserPromptSubmit", "[report][--agent][codex][--state][working][--waiting][]"),
        ("PostToolUse", "[report][--agent][codex][--state][working]"),
        ("PermissionRequest", "[report][--agent][codex][--state][blocked]"),
        ("Stop", "[report][--agent][codex][--state][idle]"),
        ("Interrupt", "[report][--agent][codex][--state][idle]"),
    ],
    turn_ends: &["Stop", "Interrupt"],
};

const HARNESSES: [Harness; 2] = [CLAUDE_CODE, CODEX];

/// Each hook command the file gives `event`, in order.
fn commands(harness: &Harness, event: &str) -> Vec<String> {
    let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
    hooks["hooks"][event]
        .as_array()
        .unwrap_or_else(|| panic!("{}: no {event} hook", harness.extras))
        .iter()
        .flat_map(|group| group["hooks"].as_array().unwrap().iter())
        .map(|hook| hook["command"].as_str().unwrap().to_string())
        .collect()
}

/// What `command` runs `$MUSTER_DAEMON` with, bracketed per argument; empty when it does not.
fn reported(command: &str, scratch: &Scratch) -> String {
    let arguments = scratch.0.join("arguments");
    let _ = std::fs::remove_file(&arguments);
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!("#!/bin/sh\nprintf '[%s]' \"$@\" > '{}'\n", arguments.display()),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ran = Command::new("/bin/sh")
        .args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", &daemon)
        .status()
        .unwrap();
    assert!(ran.success(), "a hook never fails a session: {command}");
    std::fs::read_to_string(&arguments).unwrap_or_default()
}

#[test]
fn every_hook_reports_what_its_event_means_and_every_turn_end_reports_idle() {
    let scratch = Scratch::new("harness-hooks");
    for harness in &HARNESSES {
        let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
        let mut events: Vec<&str> =
            hooks["hooks"].as_object().unwrap().keys().map(String::as_str).collect();
        let mut pinned: Vec<&str> = harness.reports.iter().map(|(event, _)| *event).collect();
        events.sort_unstable();
        pinned.sort_unstable();
        assert_eq!(events, pinned, "{}: every event its hooks answer is pinned", harness.extras);
        for (event, expected) in harness.reports {
            let said: Vec<String> = commands(harness, event)
                .iter()
                .map(|command| reported(command, &scratch))
                .filter(|said| !said.is_empty())
                .collect();
            assert_eq!(said, [*expected], "{}: {event}", harness.extras);
        }
        for event in harness.turn_ends {
            let idle = harness.reports.iter().find(|(reported, _)| reported == event);
            assert!(
                idle.is_some_and(|(_, said)| said.ends_with("[--state][idle]")),
                "{}: {event} ends a turn and does not report idle",
                harness.extras
            );
        }
    }
}

/// What the SessionStart hook with no matcher prints, which the harness adds to the session's
/// context, when run in an environment holding `variables`.
fn session_start_context(harness: &Harness, variables: &[(&str, &str)]) -> String {
    let hooks: serde_json::Value = serde_json::from_str(harness.hooks).unwrap();
    let groups = hooks["hooks"]["SessionStart"].as_array().expect("a SessionStart hook");
    let every_session = groups.iter().find(|group| group.get("matcher").is_none());
    let command = every_session.expect("one for every session")["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    let ran = Command::new("/bin/sh")
        .args(["-c", &command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(variables.iter().copied())
        .output()
        .unwrap();
    assert!(ran.status.success(), "the hook never fails a session");
    String::from_utf8(ran.stdout).unwrap()
}

/// An agent in a Muster pane is told, in one line, how to say it is waiting on its own work.
/// Nowhere else: the line costs every session some context, and outside Muster it means nothing.
#[test]
fn only_a_session_in_a_muster_pane_is_told_how_to_say_it_is_waiting() {
    for harness in &HARNESSES {
        let in_a_pane = [("MUSTER_PANE", "p1"), ("MUSTER_DAEMON", "/bin/true")];
        let told = session_start_context(harness, &in_a_pane);
        assert_eq!(told.lines().count(), 1, "{}: {told}", harness.extras);
        assert!(told.contains("report --waiting"), "{}: {told}", harness.extras);
        assert_eq!(session_start_context(harness, &[("MUSTER_DAEMON", "/bin/true")]), "");
        assert_eq!(session_start_context(harness, &[]), "");
    }
}
