//! A real ssh master to a real machine, doing the three things this crate offers.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: this needs the devenv
//! container, which a contributor's machine cannot be assumed to have running
//! (`docs/testing.md`, tiered by what a tier can reach). `./dev --ssh` brings the container up
//! and runs it, and fails if it discovers none - an ignored test that nobody notices is the
//! silently-skipped suite in a different costume.
//!
//! Nothing here is herdr-shaped and nothing here needs a daemon, which is the point: this crate
//! is a child process, a path, and the promise that the path keeps working. What a daemon does
//! once it is over there belongs to `muster-herdr`'s own devenv test.
//!
//! These write only under `/tmp` on the far machine, so they can run beside that one.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_ssh::{Forward, Tunnel};

/// Where the container is, and how to reach it.
///
/// Read from the environment rather than hardcoded, because `./dev --ssh` owns the container
/// and knows the key it generated. A test that reconstructed the arguments would be a second
/// copy of that knowledge.
fn devenv() -> (String, Vec<String>) {
    let host = std::env::var("MUSTER_DEVENV_HOST").expect(
        "MUSTER_DEVENV_HOST is unset, so this test has no machine to talk to. Run it through \
         ./dev --ssh, which starts the container and sets it.",
    );
    let options = std::env::var("MUSTER_DEVENV_SSH_OPTIONS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (host, options)
}

/// A master with a forward that nothing is listening on at the far end.
///
/// Which is the interesting case rather than a degenerate one: Muster opens the master before
/// the daemon exists, so that everything afterwards can ask "does it answer" through the
/// forwarded path instead of inventing a second way to probe.
fn master(name: &str) -> Tunnel {
    let (host, options) = devenv();
    let temporary = std::env::temp_dir();
    Tunnel::open(
        Forward {
            host,
            options,
            control_path: temporary
                .join(format!("muster-devenv-{name}.ctl"))
                .to_string_lossy()
                .into(),
            local_socket: temporary
                .join(format!("muster-devenv-{name}.sock"))
                .to_string_lossy()
                .into(),
            remote_socket: format!("/tmp/muster-devenv-{name}-nothing.sock"),
        },
        Arc::new(|_| {}),
    )
    .expect("the tunnel should open")
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_master_forwards_a_socket_before_anything_is_listening_on_it() {
    let tunnel = master("bind");
    // The claim the attach order rests on. ssh binds the local end when it connects and reaches
    // the far one per connection, so a daemon that does not exist yet costs nothing - and if
    // that ever stopped being true, Muster would have to install its daemon over a second
    // connection and this is where it would be noticed.
    assert!(
        std::path::Path::new(tunnel.local_socket_path()).exists(),
        "the near end should be bound even though the far end names nothing"
    );
    // And it stays up rather than exiting, which `ExitOnForwardFailure` would have made it do
    // if a missing far-end socket counted as a failed forward.
    std::thread::sleep(Duration::from_millis(200));
    assert!(tunnel.remote().run(&["true"]).is_ok(), "the master should still be carrying commands");
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_master_that_was_killed_comes_back_still_carrying_commands() {
    // Plugging a laptop back in flapped this connection 97 times in two minutes, and one
    // reason it could never settle is here: the control path is a file, ssh will not replace
    // one, and `-M -S <existing path>` disables multiplexing with a warning rather than
    // failing. So a master that was killed rather than asked to leave came back without its
    // multiplexing, and every pane's bridge - which rides `-S` - was then dialing a master
    // that no longer existed (kan a_2IRdZK6Un).
    //
    // Killed rather than asked to exit, deliberately: `ssh -O exit` removes the control path
    // on its way out, which is the one case that never had the bug.
    let tunnel = master("reopen");
    let pid = master_pid(&tunnel);
    assert!(tunnel.remote().run(&["true"]).is_ok(), "it should carry a command to begin with");

    assert!(
        Command::new("kill").args(["-9", &pid]).status().expect("kill runs").success(),
        "the master should have been killable"
    );

    // Carrying a command, not merely existing. A reopened master whose multiplexing was
    // disabled looks perfectly healthy from outside and answers nothing on `-S`.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if tunnel.remote().run(&["true"]).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "the tunnel to {} did not carry a command again within 20s of its master being \
         killed.\n  Impact: every pane on that machine stays dark until the app is \
         relaunched, which is what this whole path exists to avoid.\n  Check the run log for \
         `tunnel.reopen_failed`, and whether {} was left behind by the master that died - \
         `-M -S` onto a path that exists disables multiplexing instead of failing.",
        tunnel.host(),
        tunnel.control_path(),
    );
}

/// The pid of the master behind this tunnel, out of ssh's own answer.
///
/// `ssh -O check` prints `Master running (pid=NNN)` on standard error, which is the only place
/// the pid is available: the `Tunnel` owns its child and does not hand it out, deliberately.
fn master_pid(tunnel: &Tunnel) -> String {
    let checked = Command::new("ssh")
        .args(["-O", "check", "-S", tunnel.control_path(), tunnel.host()])
        .output()
        .expect("ssh runs");
    let said = String::from_utf8_lossy(&checked.stderr).into_owned();
    let Some(pid) = said.split_once("pid=").and_then(|(_, rest)| rest.split(')').next()) else {
        panic!("ssh -O check did not name a master pid, it said: {said}")
    };
    pid.to_string()
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_command_rides_the_master_and_says_what_the_machine_is() {
    let tunnel = master("platform");
    let far = tunnel.remote();

    let said = far.platform().expect("the far end should answer uname");
    assert_eq!(said.system, "Linux", "the devenv is a Linux container");
    assert!(
        said.machine == "aarch64" || said.machine == "x86_64",
        "and runs on one of the two architectures the pin carries, not {}",
        said.machine
    );

    // A failing command comes back as a refusal carrying the far end's own words, rather than
    // as an empty success. Muster decides whether a daemon is installed from an answer like
    // this one, so a failure read as "no" would reinstall on every launch.
    let refusal = far.run(&["false"]).expect_err("a command that fails should fail");
    assert!(refusal.contains("false"), "the refusal should name what was run: {refusal}");
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_file_arrives_whole_and_executable() {
    let tunnel = master("place");
    let far = tunnel.remote();
    let path = "/tmp/muster-devenv place/hello";

    // A path with a space in it, because that is the case the two layers of quoting exist for
    // and the one that would otherwise fail on somebody's machine rather than here.
    far.place(path, b"#!/bin/sh\necho placed\n", "0755").expect("the file should arrive");

    let said = far
        .shell(&format!("exec {}", muster_ssh::quoted(path)))
        .expect("the placed file should be executable over there");
    assert_eq!(said.trim(), "placed", "and should be the bytes that were sent");

    // Nothing is left at the staging name, which is what makes an interrupted copy leave no
    // path that looks like a finished install.
    assert_eq!(
        far.shell(&format!(
            "ls {}.placing >/dev/null 2>&1 && echo yes || echo no",
            muster_ssh::quoted(path)
        ))
        .expect("the far end should answer")
        .trim(),
        "no",
        "the staged name should have been renamed away"
    );

    let _ = far.shell("rm -rf '/tmp/muster-devenv place'");
}
