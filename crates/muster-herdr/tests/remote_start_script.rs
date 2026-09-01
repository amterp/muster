//! The script that starts a daemon on another machine, run against a shell that forks.
//!
//! kan a_2HpkpQlZP. `&` applied to the whole `&&` list rather than to the `nohup`, so a far
//! shell that forks ran the list in a subshell and waited on herdr inside it. The redirect
//! covered herdr's descriptors and not the subshell's, so that subshell held ssh's stdout and
//! stderr for as long as the daemon lived - and `Remote::shell` is a `wait_with_output` that
//! wants end of file on both. Measured against a RHEL-family box: four minutes blocked in
//! `start`, and a Quit that had been queued behind it fired the instant the block cleared.
//!
//! **bash rather than `/bin/sh`, deliberately.** Whether this happens is decided by which shell
//! is over there: bash forks, and dash execs the last command of a backgrounded list, which
//! replaces the subshell with the redirected process and closes the pipes. `/bin/sh` is bash on
//! macOS and dash on Debian, so reaching it through that name would make this test pass
//! vacuously on the Linux runner. The devenv container's `/bin/sh` is dash, which is why the
//! daemon-backed remote tier cannot see this at all.
//!
//! What is not proven here is the attach end to end, which needs a remote whose `/bin/sh` forks
//! and therefore `./dev --ssh` against an image built for it (kan a_2I5yedXos).

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use muster_herdr::remote::start_script;

/// Long enough that a real hang cannot be mistaken for a slow machine, short enough that a
/// failing run says so rather than looking wedged. The fixed script returns in milliseconds.
const ALLOWANCE: Duration = Duration::from_secs(10);

#[test]
fn the_shell_that_starts_the_daemon_lets_go_of_ssh() {
    let scratch = scratch("lets-go");
    let daemon = fake_daemon(&scratch);
    let output = scratch.join("herdr.out");

    let script = start_script(
        &scratch.display().to_string(),
        &daemon.display().to_string(),
        &scratch.join("herdr.toml").display().to_string(),
        &output.display().to_string(),
    );

    let closed = read_to_end_under_bash(&script);
    // Polled rather than read once: the fixed script lets go of the pipes as soon as it has
    // forked, which is before the daemon behind it has run its first line. That order is the
    // point of the fix, so the check has to allow for it.
    let started = until(|| scratch.join("started").exists());
    stop(&scratch);

    assert!(
        closed,
        "the shell that started the daemon was still holding ssh's stdout and stderr {} seconds \
         later. `Remote::shell` waits for end of file on both, so an attach against a machine \
         whose /bin/sh forks blocks in `start` until something kills that shell - four minutes, \
         measured - and every AppleEvent queued behind it, a Quit included, fires the moment it \
         clears. The script was:\n  {script}",
        ALLOWANCE.as_secs()
    );
    assert!(
        started,
        "the shell let go of the pipes and started nothing. A script that backgrounds no daemon \
         passes the check above and leaves that machine's panes absent from the window. What it \
         wrote to the file a daemon that never bound says why in:\n{}",
        std::fs::read_to_string(&output).unwrap_or_else(|_| "(no such file)".to_string())
    );
}

/// Waits for something to become true, or gives up.
///
/// The only asynchrony here: the script's whole purpose is to return before the daemon it
/// started has done anything.
fn until(ready: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + ALLOWANCE;
    while std::time::Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Runs the script the way `Remote::shell` runs it, and says whether both pipes reached end of
/// file in time.
///
/// `wait_with_output` in the real path, and a read to end of file here, which is the same
/// question: whether anything is still holding the descriptors ssh handed over.
fn read_to_end_under_bash(script: &str) -> bool {
    let mut child = Command::new("bash")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bash should be on PATH: it is on macOS and on the Linux runner");
    let mut streams = vec![
        Box::new(child.stdout.take().expect("piped")) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().expect("piped")) as Box<dyn Read + Send>,
    ];

    let (done, closed) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in &mut streams {
            let mut sink = Vec::new();
            let _ = stream.read_to_end(&mut sink);
        }
        let _ = done.send(());
    });

    let reached_eof = closed.recv_timeout(ALLOWANCE).is_ok();
    let _ = child.kill();
    let _ = child.wait();
    reached_eof
}

/// A program that records that it ran and then stays up, the way a daemon does.
///
/// Staying up is the whole fixture: a daemon that exited would close the pipes on its way out
/// and the broken script would pass. It polls for a file rather than sleeping a fixed time so
/// the test can end it, and gives up on its own so a panicking test leaves nothing behind.
fn fake_daemon(scratch: &Path) -> PathBuf {
    let script = scratch.join("fake-herdr");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             printf '' > {started}\n\
             i=0\n\
             while [ ! -f {stop} ] && [ $i -lt 200 ]; do sleep 0.1; i=$((i+1)); done\n",
            started = scratch.join("started").display(),
            stop = scratch.join("stop").display(),
        ),
    )
    .expect("the scratch root should be writable");
    let mut mode = std::fs::metadata(&script).expect("just written").permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(&script, mode).expect("the scratch root should be writable");
    script
}

fn stop(scratch: &Path) {
    let _ = std::fs::write(scratch.join("stop"), b"");
}

fn scratch(name: &str) -> PathBuf {
    let path =
        PathBuf::from(format!("/tmp/muster-test/remote-start-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("state")).expect("the harness root should be writable");
    path
}
