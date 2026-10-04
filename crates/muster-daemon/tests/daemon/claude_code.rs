//! The Claude Code statusline in `extras/claude-code/`, run as Claude Code runs it. Its hooks,
//! and every other harness's, are `harness_hooks`'.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::support::*;

const STATUSLINE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../extras/claude-code/statusline.sh");

/// What Claude Code hands its statusline command, trailing newline and all.
const STATUS: &str = "{\"model\":{\"display_name\":\"Opus\"},\"context_window\":\
                      {\"used_percentage\":42},\"cost\":{\"total_cost_usd\":1.5}}\n";

pub(super) struct Scratch(pub(super) PathBuf);

impl Scratch {
    pub(super) fn new(name: &str) -> Scratch {
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

/// The statusline reports through the real `muster-daemon report`, which reads Claude Code's JSON
/// itself: with no jq anywhere on the PATH, the pane still takes the context, model and cost, and
/// the session's name - and a session that says it has no name leaves the pane's name alone.
#[test]
fn the_statusline_reports_the_session_with_no_jq_installed() {
    let scratch = Scratch::new("statusline-report");
    // Only what the script runs besides the report: `cat`.
    let bin = scratch.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink("/bin/cat", bin.join("cat")).unwrap();
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let run = |status: &str| {
        // `cat` after it draws the line: drawing it with nothing after it is what needs jq.
        let mut statusline = Command::new("/bin/sh")
            .args([STATUSLINE, "cat"])
            .env_clear()
            .env("PATH", &bin)
            .env("MUSTER_DAEMON", env!("CARGO_BIN_EXE_muster-daemon"))
            .env("MUSTER_DAEMON_SOCKET", daemon.socket_path())
            .env("MUSTER_PANE", "p1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("sh runs the statusline");
        statusline.stdin.take().unwrap().write_all(status.as_bytes()).unwrap();
        assert!(statusline.wait().unwrap().success());
    };
    let pane = |control: &mut Control| {
        snapshot(control).panes.into_iter().find(|record| record.pane == "p1").unwrap()
    };

    run(&STATUS.replace("{\"model\"", "{\"session_name\":\"🤖 A\",\"model\""));
    let named = until_some("the report to land", || {
        let record = pane(&mut control);
        record
            .facts
            .as_ref()
            .is_some_and(|facts| facts.context_used == Some(42.0))
            .then_some(record)
    });
    let facts = named.facts.unwrap();
    assert_eq!((facts.model.as_deref(), facts.cost_usd), (Some("Opus"), Some(1.5)));
    assert_eq!(named.label.as_deref(), Some("🤖 A"), "the pane took the session's name");

    run(&STATUS.replace("42", "50"));
    let unnamed = until_some("the second report to land", || {
        let record = pane(&mut control);
        record
            .facts
            .as_ref()
            .is_some_and(|facts| facts.context_used == Some(50.0))
            .then_some(record)
    });
    assert_eq!(unnamed.label.as_deref(), Some("🤖 A"), "no name took the pane's away");
}
