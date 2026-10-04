//! The hooks each harness's wiring in `extras/` installs, run as the harness runs them: what each
//! event reports to the daemon owning the pane. A harness's hooks are the only way its own word
//! reaches Muster, so every event a file names is pinned here, and every event that ends a turn
//! must report idle - an agent that declared a wait is moved on only by its own idle report.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::Duration;

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

/// The shells a harness may run a hook command in: `sh`, and the user's own, which on a Mac is
/// zsh - where an unquoted expansion is not split into words, as it is in `sh`. Codex runs its
/// hooks in the user's shell.
fn shells() -> Vec<&'static str> {
    ["/bin/sh", "/bin/zsh"]
        .into_iter()
        .filter(|shell| std::path::Path::new(shell).exists())
        .collect()
}

/// What `command` runs `$MUSTER_DAEMON` with, bracketed per argument, in each of [`shells`],
/// which must agree; empty when it does not run it.
fn reported(command: &str, scratch: &Scratch) -> String {
    let said: Vec<String> =
        shells().iter().map(|shell| reported_in(shell, command, scratch)).collect();
    assert!(
        said.windows(2).all(|pair| pair[0] == pair[1]),
        "the shells disagree on {command}: {said:?}"
    );
    said.into_iter().next().unwrap_or_default()
}

fn reported_in(shell: &str, command: &str, scratch: &Scratch) -> String {
    let arguments = scratch.0.join("arguments");
    let _ = std::fs::remove_file(&arguments);
    let daemon = scratch.0.join("daemon");
    // Written aside and moved into place: a test waits for the file to appear, and a redirect
    // makes it before printf fills it.
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" > '{0}.part' && mv '{0}.part' '{0}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ran = Command::new(shell)
        .args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", &daemon)
        .stdin(Stdio::null())
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

/// The tail of a Codex 0.154.0 rollout transcript: two token counts with an answer between them.
const ROLLOUT_TAIL: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/codex-0.154.0/rollout-tail.jsonl");

/// Codex has no statusline, so its hooks read how full its context is from the last token count
/// in its transcript, as Codex's own "N% context left" counts it: past a baseline of 12,000
/// tokens it does not count as used. The last of the tail's counts is 19,313 tokens of 258,400.
#[test]
fn codexs_hooks_report_its_context_from_its_transcripts_last_token_count() {
    let scratch = Scratch::new("codex-context");
    let arguments = scratch.0.join("arguments");
    let daemon = scratch.0.join("daemon");
    // Written aside and moved into place: a test waits for the file to appear, and a redirect
    // makes it before printf fills it.
    std::fs::write(
        &daemon,
        format!(
            "#!/bin/sh\nprintf '[%s]' \"$@\" > '{0}.part' && mv '{0}.part' '{0}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({ "transcript_path": ROLLOUT_TAIL, "model": "gpt-5.6-luna" });
    for (event, shell) in ["PostToolUse", "Stop"]
        .into_iter()
        .flat_map(|event| shells().into_iter().map(move |shell| (event, shell)))
    {
        let context = commands(&CODEX, event)
            .into_iter()
            .find(|command| command.contains("token_count"))
            .unwrap_or_else(|| panic!("no {event} hook reads the context"));
        let _ = std::fs::remove_file(&arguments);
        let mut hook = Command::new(shell)
            .args(["-c", &context])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MUSTER_DAEMON", &daemon)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        assert!(hook.wait().unwrap().success(), "a hook never fails a session");
        muster_harness::until_within(
            "the context report, which runs in the background",
            Duration::from_secs(20),
            || arguments.exists(),
            || format!("{event}: nothing reported"),
        );
        let said = std::fs::read_to_string(&arguments).unwrap();
        assert!(said.starts_with("[report][--context-used][2.96"), "{event} in {shell}: {said}");
        assert!(said.ends_with("[--model][gpt-5.6-luna]"), "{event} in {shell}: {said}");
    }
}

/// Codex is woken through `codex queue` by its session's id, which only its hooks are handed:
/// `SessionStart` reports it, and says nothing to the model doing so.
#[test]
fn codexs_session_start_reports_its_session_id() {
    let scratch = Scratch::new("codex-session-id");
    let arguments = scratch.0.join("arguments");
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!("#!/bin/sh\nprintf '[%s]' \"$@\" >> '{}'\n", arguments.display()),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({ "session_id": "019a-b c", "source": "startup" });
    let reporting = commands(&CODEX, "SessionStart")
        .into_iter()
        .find(|command| command.contains("--session-id"))
        .expect("a SessionStart hook reports the session's id");
    for shell in shells() {
        let _ = std::fs::remove_file(&arguments);
        let mut hook = Command::new(shell)
            .args(["-c", &reporting])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("MUSTER_DAEMON", &daemon)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        hook.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let output = hook.wait_with_output().unwrap();
        assert!(output.status.success(), "a hook never fails a session");
        assert!(output.stdout.is_empty(), "{shell}: what it prints reaches the model");
        let said = std::fs::read_to_string(&arguments).unwrap_or_default();
        assert_eq!(said, "[report][--agent][codex][--session-id][019a-b c]", "in {shell}");
    }
}

const CODEX_MESSAGING: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/codex/messaging-hooks.json"));

/// Codex hands the model a hook's `additionalContext`, where a `PostToolUse` hook's exit 2 would
/// replace the tool's result (docs/observations/codex-0.154.0.md, section 7): what its messaging
/// hooks fetch reaches it that way, and nothing at all when nothing is unread.
#[test]
fn codexs_messaging_hooks_hand_what_arrived_to_the_model_as_context() {
    let scratch = Scratch::new("codex-messaging");
    let hooks: serde_json::Value = serde_json::from_str(CODEX_MESSAGING).unwrap();
    let unread = scratch.0.join("unread");
    let muster = scratch.0.join("muster");
    std::fs::write(&muster, format!("#!/bin/sh\ncat '{}' 2>/dev/null\n", unread.display()))
        .unwrap();
    std::fs::set_permissions(&muster, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |command: &str| {
        let ran = Command::new("/bin/sh")
            .args(["-c", command])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", scratch.0.display()))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(ran.status.success(), "a hook never fails a session: {command}");
        String::from_utf8(ran.stdout).unwrap()
    };
    for event in ["UserPromptSubmit", "PostToolUse"] {
        let command = hooks["hooks"][event][0]["hooks"][0]["command"].as_str().unwrap();
        let _ = std::fs::remove_file(&unread);
        assert_eq!(run(command), "", "{event}: something said with nothing unread");
        std::fs::write(&unread, "#4 director: the schema changed\n").unwrap();
        let said: serde_json::Value = serde_json::from_str(&run(command)).unwrap();
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], event);
        assert_eq!(
            said["hookSpecificOutput"]["additionalContext"],
            "#4 director: the schema changed"
        );
    }
}
