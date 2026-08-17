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

use std::time::Duration;

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
    Tunnel::open(Forward {
        host,
        options,
        control_path: temporary.join(format!("muster-devenv-{name}.ctl")).to_string_lossy().into(),
        local_socket: temporary.join(format!("muster-devenv-{name}.sock")).to_string_lossy().into(),
        remote_socket: format!("/tmp/muster-devenv-{name}-nothing.sock"),
    })
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
