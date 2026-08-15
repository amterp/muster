//! A real ssh master to a real machine, forwarding a real daemon's socket.
//!
//! Out of the default gate, and marked `#[ignore]` to keep it there: this needs the devenv
//! container, which a contributor's machine cannot be assumed to have running
//! (`docs/testing.md`, tiered by what a tier can reach). `./dev --ssh` brings the container
//! up and runs it, and fails if it discovers none - an ignored test that nobody notices is
//! the silently-skipped suite in a different costume.
//!
//! What it proves is the one claim the whole remote arc rests on: a forwarded socket is a
//! socket. Nothing here is herdr-shaped, and nothing herdr-shaped needs to change for a
//! daemon to be on another machine.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use muster_ssh::{Forward, Tunnel, remote_environment};

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

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_remote_daemon_answers_through_a_forwarded_socket() {
    let (host, options) = devenv();

    // Where the daemon is over there, worked out the way Muster works it out: ask the far end
    // what its environment looks like and apply herdr's own rules to it. A hardcoded path
    // here would pass while the code that matters was wrong.
    let environment = remote_environment(&host, &options).expect("the devenv should answer ssh");
    let remote = muster_herdr::discover_socket_path(&environment)
        .expect("the container's environment should say where herdr keeps its socket");

    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(Forward {
        host: host.clone(),
        options,
        control_path: temporary.join("muster-devenv-test.ctl").to_string_lossy().into_owned(),
        local_socket: temporary.join("muster-devenv-test.sock").to_string_lossy().into_owned(),
        remote_socket: remote,
    })
    .expect("the tunnel should open");

    // The whole claim, in one request: a plain unix socket client, speaking herdr's ordinary
    // JSON, against a daemon on another machine. If this needs anything the local path does
    // not, "local and remote in one window" costs more than the transport.
    let answer =
        ask(tunnel.local_socket_path(), r#"{"id":"t1","method":"session.snapshot","params":{}}"#);
    assert!(
        answer.contains("\"result\""),
        "the forwarded socket should answer a snapshot, and said: {answer}"
    );
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_pane_s_frames_come_through_the_master() {
    let (host, options) = devenv();
    let environment = remote_environment(&host, &options).expect("the devenv should answer ssh");
    let remote = muster_herdr::discover_socket_path(&environment)
        .expect("the container's environment should say where herdr keeps its socket");

    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(Forward {
        host: host.clone(),
        options: options.clone(),
        control_path: temporary.join("muster-devenv-frames.ctl").to_string_lossy().into_owned(),
        local_socket: temporary.join("muster-devenv-frames.sock").to_string_lossy().into_owned(),
        remote_socket: remote,
    })
    .expect("the tunnel should open");

    // The data plane cannot ride the forwarded socket - herdr publishes frames through its
    // CLI rather than through a socket method - so a pane runs a command over the master
    // instead. What this asserts is that the master accepts one, which is what makes a remote
    // pane cost no handshake of its own.
    let ran = std::process::Command::new("ssh")
        .args(["-S", tunnel.control_path(), "-o", "BatchMode=yes", &host, "herdr", "--version"])
        .output()
        .expect("ssh should run");
    assert!(
        ran.status.success(),
        "a command over the master should run, and said: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("herdr"),
        "the far end should have a herdr to run, and said: {}",
        String::from_utf8_lossy(&ran.stdout)
    );
}

/// One request, one answer, over a plain unix socket.
///
/// Hand-rolled rather than borrowed from the adapter, so that what is being judged is the
/// socket rather than Muster's client: a test that used `HerdrClient` would pass if the
/// client had learned to work around a forward that did not.
fn ask(socket_path: &str, request: &str) -> String {
    let mut stream = UnixStream::connect(socket_path).expect("the forwarded socket should accept");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("a timeout should be settable");
    stream.write_all(format!("{request}\n").as_bytes()).expect("the request should go");
    stream.shutdown(std::net::Shutdown::Write).expect("herdr waits for end-of-write");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
}
