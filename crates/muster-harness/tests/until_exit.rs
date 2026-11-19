//! Waiting for a child process ends at a deadline, and takes the child with it.

use std::process::{Command, Stdio};
use std::time::Duration;

use muster_harness::{until_exit, until_exit_within};

#[test]
fn a_child_that_exits_hands_back_its_status() {
    let child = Command::new("/bin/sh").args(["-c", "exit 3"]).spawn().unwrap();
    assert_eq!(until_exit("sh", child).code(), Some(3));
}

/// The hang a hook reading its stdin to the end caused when nobody closed the other end: here
/// the test holds the write end open, as an inherited stdin did.
#[test]
fn a_child_blocked_on_a_stdin_that_never_ends_fails_the_wait_and_is_killed() {
    let mut child = Command::new("/bin/cat").stdin(Stdio::piped()).spawn().unwrap();
    let _held_open = child.stdin.take();
    let pid = child.id() as libc::pid_t;

    let failed = std::panic::catch_unwind(|| {
        until_exit_within("cat", Duration::from_millis(300), child);
    })
    .expect_err("cat never sees the end of its stdin");
    let message = failed.downcast_ref::<String>().expect("a formatted panic");
    assert!(message.contains("waiting for cat to exit"), "{message}");
    assert!(message.contains("has been killed"), "{message}");
    // SAFETY: kill with signal 0 only asks whether the process exists; it was reaped, so it does not.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "cat outlived the wait");
}
