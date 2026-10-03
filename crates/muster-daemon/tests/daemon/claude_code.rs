//! The Claude Code statusline in `extras/claude-code/`, run as Claude Code runs it. Its hooks,
//! and every other harness's, are `harness_hooks`'.

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
