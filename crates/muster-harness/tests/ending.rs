//! Nothing a test starts outlives it, however the test ends: passing, panicking, or killed
//! without a chance to clean up, alone or with its whole process group.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use muster_harness::requests::pane_with_pid;
use muster_harness::{Daemon, built_daemon};

/// How long what a killed test started may take to go: the daemon's grace for a pane that
/// ignores its hang-up, three seconds, and time for the daemon to exit after.
const GONE_WITHIN: Duration = Duration::from_secs(8);

fn running(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Which of `pids` are still running once `within` has passed, killing each so that a failed
/// run leaves nothing behind. Every pid here is one this test started.
fn left_after(pids: &[i32], within: Duration) -> Vec<i32> {
    let deadline = Instant::now() + within;
    while pids.iter().any(|&pid| running(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let left: Vec<i32> = pids.iter().copied().filter(|&pid| running(pid)).collect();
    for &pid in &left {
        // SAFETY: kill signals a process this test started and found still running.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    left
}

/// Starts the owner, which starts daemons with panes, and returns it with every pid it said.
fn owner() -> (std::process::Child, Vec<i32>) {
    let mut owner = Command::new(env!("CARGO_BIN_EXE_muster-harness-owner"))
        .arg(built_daemon())
        .process_group(0)
        .stdout(Stdio::piped())
        .spawn()
        .expect("the owner starts");
    let mut pids = Vec::new();
    for line in BufReader::new(owner.stdout.take().unwrap()).lines() {
        let line = line.expect("the owner's output reads");
        if line == "ready" {
            return (owner, pids);
        }
        if let Some(pid) = line.strip_prefix("pid ") {
            pids.push(pid.parse().expect("a pid"));
        }
    }
    let status = owner.wait();
    let left = left_after(&pids, Duration::ZERO);
    panic!("the owner ended with {status:?} before it was ready; killed what it left: {left:?}");
}

fn assert_all_gone(pids: &[i32], how: &str) {
    let left = left_after(pids, GONE_WITHIN);
    assert!(
        left.is_empty(),
        "{how}: {left:?} of {pids:?} were still running {GONE_WITHIN:?} later, and were \
         killed.\n  Impact: a killed test run leaves daemons or pane shells running for good.\n  \
         Check the daemon's MUSTER_DAEMON_ENDS_WITH watch, and that the harness starts each \
         daemon in a process group of its own."
    );
}

#[test]
fn what_a_killed_test_started_ends_with_it() {
    let (mut owner, pids) = owner();
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert_all_gone(&pids, "after SIGKILL to the test process");
}

/// What `tools/run-tests.py` does to a job when the run is stopped.
#[test]
fn what_a_test_killed_with_its_process_group_started_ends_with_it() {
    let (mut owner, pids) = owner();
    // SAFETY: the group is the owner's own, which this test made with `process_group(0)`.
    unsafe { libc::killpg(owner.id().cast_signed(), libc::SIGKILL) };
    owner.wait().unwrap();
    assert_all_gone(&pids, "after SIGKILL to the test's process group");
}

/// A test that panics drops its daemon with panes still open, one of which may ignore its
/// hang-up.
#[test]
fn a_dropped_daemon_takes_a_pane_that_ignores_the_hang_up_with_it() {
    let daemon = Daemon::start_built();
    let pane = pane_with_pid(&mut daemon.connect(), "p1", true);
    drop(daemon);
    // Not at once: the daemon has exited, but its pane may wait a moment for launchd to reap it.
    let left = left_after(&[pane], Duration::from_secs(1));
    assert!(left.is_empty(), "pid {pane} outlived its daemon");
}
