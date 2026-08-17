//! Muster's own daemon, on a machine that had none.
//!
//! Out of the default gate and marked `#[ignore]` to keep it there: this needs the devenv
//! container, which a contributor's machine cannot be assumed to have running
//! (`docs/testing.md`, tiered by what a tier can reach). `./dev --ssh` brings it up and runs
//! this against it, before anything else has put a daemon in there.
//!
//! **The fixture is the absence.** The devenv image installs no herdr, because a container that
//! arrived with one would exercise the adopt path and never the install path - and a person
//! setting up a real devenv installs nothing either. What this proves is the claim the whole
//! remote arc rests on: point Muster at a machine with nothing on it, and a pane on that machine
//! works.
//!
//! One test rather than several, and that is deliberate. `./dev --ssh` runs the ignored tests
//! across the workspace in parallel, and everything here mutates one container's `~/.muster` -
//! so a second test asserting a second thing would be a race whose failures read as flakiness in
//! the code under test.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use muster_herdr::daemon::Reached;
use muster_herdr::{own_socket_path, pinned, remote};
use muster_ssh::{Forward, Tunnel, remote_environment};

/// Where the container is, and how to reach it.
///
/// Read from the environment rather than hardcoded, because `./dev --ssh` owns the container and
/// knows the key it generated. A test that reconstructed the arguments would be a second copy of
/// that knowledge.
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

/// Where the pinned daemon for the container's platform already sits on this machine.
///
/// Handed over rather than downloaded here. `./dev --ssh` fetches it once and verifies it against
/// `deps/herdr.pin`, which is where every other acquisition in this repo happens - and it keeps
/// the tier offline, which `docs/testing.md` asks of every tier rather than only of the gate.
fn cache() -> String {
    std::env::var("MUSTER_DEVENV_CACHE").expect(
        "MUSTER_DEVENV_CACHE is unset, so this test has no daemon to install and would reach \
         the network for one. Run it through ./dev --ssh, which fetches the container's \
         platform asset against the pin and points this at it.",
    )
}

#[test]
#[ignore = "needs the devenv container; run through ./dev --ssh"]
fn a_machine_with_nothing_on_it_ends_up_running_musters_own_daemon() {
    let (host, options) = devenv();
    let environment = remote_environment(&host, &options).expect("the devenv should answer ssh");
    let version = pinned().expect("the pin should parse").version;

    // Where Muster's daemon listens over there, worked out the way Muster works it out. A
    // hardcoded path here would pass while the code that matters was wrong.
    let remote_socket = own_socket_path(&environment)
        .expect("the container's environment should say where a herdr socket would go");

    let temporary = std::env::temp_dir();
    let tunnel = Tunnel::open(Forward {
        host: host.clone(),
        options,
        control_path: temporary.join("muster-devenv-install.ctl").to_string_lossy().into_owned(),
        local_socket: temporary.join("muster-devenv-install.sock").to_string_lossy().into_owned(),
        remote_socket: remote_socket.clone(),
    })
    .expect("the tunnel should open against a machine with no daemon on it");
    let far = tunnel.remote();

    // Back to nothing first, rather than trusting the container to be fresh. A run that
    // crashed half way leaves an install behind, and a test that then adopted it would report
    // success about the path it exists to check.
    // `pkill -x` matches the process name rather than the command line. `-f` would match
    // this very ssh session, whose command line necessarily contains the pattern, and kill
    // the shell running it - which surfaces as ssh exiting 255 with nothing on stderr.
    far.shell("pkill -x herdr >/dev/null 2>&1; rm -rf \"$HOME/.muster\"; true")
        .expect("the far end should let its own home be cleared");
    assert_eq!(
        far.shell("command -v herdr >/dev/null 2>&1 && echo yes || echo no")
            .expect("the far end should answer")
            .trim(),
        "no",
        "the premise of this whole test is a devenv with no daemon installed - if the image \
         gained one back, this passes while proving nothing"
    );

    let started =
        remote::ensure_running(&far, &environment, tunnel.local_socket_path(), Some(&cache()))
            .expect("Muster should put a daemon on a machine that has none");
    assert_eq!(started, Reached::Started, "there was nothing to adopt");

    // The daemon is Muster's: its own binary, at its own version, on its own herdr session.
    assert_eq!(
        far.shell(&format!(
            "test -x \"$HOME/.muster/herdr/{version}/herdr\" && echo yes || echo no"
        ))
        .expect("the far end should answer")
        .trim(),
        "yes",
        "the pinned daemon should be installed at a version-named path"
    );
    assert!(
        remote_socket.contains("/sessions/muster/"),
        "Muster's daemon should listen on a session of its own over there too, and listens on \
         {remote_socket}"
    );

    // A plain unix socket client speaking herdr's ordinary JSON, against a daemon on another
    // machine that Muster itself put there. Hand-rolled rather than borrowed from the adapter,
    // so what is judged is the socket rather than Muster's client.
    let answer =
        ask(tunnel.local_socket_path(), r#"{"id":"t1","method":"session.snapshot","params":{}}"#);
    assert!(
        answer.contains("\"result\""),
        "the forwarded socket should answer a snapshot, and said: {answer}"
    );

    // What a pane's bridge will run: the CLI Muster installed, found at a path that does not
    // carry a version, talking to the daemon it was told about rather than one it looked for.
    // Without this a remote pane renders nothing and says nothing.
    let ran = far
        .shell(&format!(
            "HERDR_SOCKET_PATH={remote_socket} \"$HOME/.muster/bin/herdr\" pane list >/dev/null \
             2>&1 && echo yes || echo no"
        ))
        .expect("the far end should answer");
    assert_eq!(
        ran.trim(),
        "yes",
        "the herdr Muster installed should be runnable at ~/.muster/bin/herdr and should reach \
         the daemon Muster started"
    );

    // Asked a second time, a daemon that is already holding somebody's agents is reused. That
    // is "sessions outlive the app" at one machine's remove, and it is also what keeps a second
    // launch from paying for the install again.
    let adopted =
        remote::ensure_running(&far, &environment, tunnel.local_socket_path(), Some(&cache()))
            .expect("the second attach should find the daemon it started");
    assert_eq!(adopted, Reached::Adopted, "the daemon was already answering");

    // Left as it was found, so the corpus probe that runs after this starts its own daemon
    // rather than meeting one nobody expected.
    far.shell("pkill -x herdr >/dev/null 2>&1; rm -rf \"$HOME/.muster\"; true")
        .expect("the far end should let its own home be cleared");
}

/// One request, one answer, over a plain unix socket.
fn ask(socket_path: &str, request: &str) -> String {
    let mut stream = UnixStream::connect(socket_path).expect("the forwarded socket should accept");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("a timeout should be settable");
    stream.write_all(format!("{request}\n").as_bytes()).expect("the request should go");
    stream.shutdown(std::net::Shutdown::Write).expect("herdr waits for end-of-write");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
}
