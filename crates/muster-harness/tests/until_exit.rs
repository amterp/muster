//! Waiting for a child process ends at a deadline, and takes the child's whole group with it.

use std::process::{Command, Stdio};
use std::time::Duration;

use muster_harness::{until, until_exit, until_exit_within};

#[test]
fn a_child_that_exits_hands_back_its_status() {
    let status = until_exit("sh", Command::new("/bin/sh").args(["-c", "exit 3"]));
    assert_eq!(status.code(), Some(3));
}

/// The hang a Claude Code hook caused when nobody closed its stdin: a shell waiting on the `cat`
/// it started for `$(cat)`. The test holds the write end of the stdin for as long as it runs, as
/// whatever launched the suite did, so the cat can only end by being killed.
#[test]
fn a_hook_blocked_on_a_stdin_that_never_ends_fails_the_wait_and_its_group_is_killed() {
    let said_pid = std::env::temp_dir().join(format!("until-exit-{}", std::process::id()));
    let script = format!("echo $$ > '{}'; case \"$(cat)\" in *) ;; esac", said_pid.display());
    let (stdin, _held_open) = std::io::pipe().unwrap();
    let mut hook = Command::new("/bin/sh");
    hook.args(["-c", &script]).stdin(Stdio::from(stdin));

    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        until_exit_within("the hook", Duration::from_millis(300), &mut hook);
    }))
    .expect_err("cat never sees the end of its stdin");
    let message = failed.downcast_ref::<String>().expect("a formatted panic");
    assert!(message.contains("waiting for the hook to exit"), "{message}");
    assert!(message.contains("has been killed"), "{message}");

    let group: libc::pid_t = std::fs::read_to_string(&said_pid).unwrap().trim().parse().unwrap();
    let _ = std::fs::remove_file(&said_pid);
    // The killed cat is an orphan until init reaps it, and a zombie still counts in its group.
    // SAFETY: killpg with signal 0 only asks whether any process is left in the group.
    until("the shell's cat to be gone", || unsafe { libc::killpg(group, 0) } == -1, ());
}
