//! Starts test daemons with panes the way a test does, says the pid of every process it started,
//! and waits to be killed: the process `tests/ending.rs` kills without letting it clean up.
//!
//! Usage: muster-harness-owner <muster-daemon>
//!
//! Prints one `pid <pid>` line per daemon and pane shell, then `ready`.

use std::io::Write;

use muster_harness::Daemon;
use muster_harness::requests::{IGNORING_THE_HANG_UP, pane_with_pid};

fn main() {
    let binary = std::env::args().nth(1).expect("usage: muster-harness-owner <muster-daemon>");
    let mut pids = Vec::new();

    let first = Daemon::start(&binary);
    let mut control = first.connect();
    pids.push(pane_with_pid(&mut control, "p1", "while :; do sleep 1; done"));
    pids.push(pane_with_pid(&mut control, "p2", IGNORING_THE_HANG_UP));
    pids.push(first.pid().cast_signed());

    // A handoff's successor is started by the daemon, in a session of its own, so it is
    // neither this process's child nor in its process group.
    let mut handed_over = Daemon::start(&binary);
    pids.push(pane_with_pid(&mut handed_over.connect(), "p3", IGNORING_THE_HANG_UP));
    let answer = handed_over.replace(None);
    assert_eq!(
        answer.outcome(),
        muster_daemon_proto::Outcome::Done,
        "the handoff: {}",
        answer.reason
    );
    pids.push(handed_over.pid().cast_signed());

    let mut out = std::io::stdout().lock();
    for pid in pids {
        writeln!(out, "pid {pid}").unwrap();
    }
    writeln!(out, "ready").unwrap();
    out.flush().unwrap();
    drop(out);
    loop {
        std::thread::park();
    }
}
