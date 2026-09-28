//! The Claude Code wiring in `extras/claude-code/`, run as Claude Code runs it.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const STATUSLINE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code/statusline.sh");

/// What Claude Code hands its statusline command, trailing newline and all.
const STATUS: &str = "{\"model\":{\"display_name\":\"Opus\"},\"context_window\":\
                      {\"used_percentage\":42},\"cost\":{\"total_cost_usd\":1.5}}\n";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("muster-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the temp directory is writable");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Claude Code reads the statusline until every writer has closed its pipe, so a report still
/// running with the pipe open holds the line up for as long as the daemon takes to answer.
#[test]
fn a_daemon_that_never_answers_never_holds_the_statusline_up() {
    let scratch = Scratch::new("statusline");
    let reported = scratch.0.join("reported");
    let daemon = scratch.0.join("daemon");
    std::fs::write(&daemon, format!("#!/bin/sh\ntouch '{}'\nsleep 3\n", reported.display()))
        .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let mut statusline = Command::new("/bin/sh")
        .args([STATUSLINE, "cat"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", &daemon)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sh runs the statusline");
    statusline.stdin.take().unwrap().write_all(STATUS.as_bytes()).unwrap();
    let output = statusline.wait_with_output().unwrap();
    let took = started.elapsed();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        STATUS,
        "the wrapped command gets Claude Code's input byte for byte"
    );
    assert!(took < Duration::from_secs(1), "the statusline took {took:?}");
    // A minute, because the fake daemon is a file macOS has never run, and it holds a new
    // executable's first run while it scans it: 23 s has been measured on a loaded machine.
    muster_harness::until_within(
        "the report to run, without which this proved nothing",
        Duration::from_mins(1),
        || reported.exists(),
        || format!("{} was never touched", reported.display()),
    );
}

const HOOKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code/hooks/hooks.json"));

/// What the SessionStart hook with no matcher prints, which Claude Code adds to the session's
/// context, when run in an environment holding `variables`.
fn session_start_context(variables: &[(&str, &str)]) -> String {
    let hooks: serde_json::Value = serde_json::from_str(HOOKS).unwrap();
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
    let told = session_start_context(&[("MUSTER_PANE", "p1"), ("MUSTER_DAEMON", "/bin/true")]);
    assert_eq!(told.lines().count(), 1, "{told}");
    assert!(told.contains("report --waiting"), "{told}");
    assert_eq!(session_start_context(&[("MUSTER_DAEMON", "/bin/true")]), "");
    assert_eq!(session_start_context(&[]), "");
}

/// A person's prompt ends any wait the agent declared: whatever it was waiting on, the person
/// has moved it on.
#[test]
fn a_prompt_reports_working_and_ends_a_wait() {
    let scratch = Scratch::new("prompt-hook");
    let arguments = scratch.0.join("arguments");
    let daemon = scratch.0.join("daemon");
    std::fs::write(
        &daemon,
        format!("#!/bin/sh\nprintf '[%s]' \"$@\" > '{}'\n", arguments.display()),
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hooks: serde_json::Value = serde_json::from_str(HOOKS).unwrap();
    let command = hooks["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"].as_str().unwrap();

    let ran = Command::new("/bin/sh")
        .args(["-c", command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("MUSTER_DAEMON", &daemon)
        .status()
        .unwrap();

    assert!(ran.success());
    assert_eq!(
        std::fs::read_to_string(&arguments).unwrap(),
        "[report][--agent][claude][--state][working][--waiting][]"
    );
}
