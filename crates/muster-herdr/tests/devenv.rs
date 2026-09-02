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

use herdr_harness::until_some;
use muster_core::config;
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::names::{Mint, Names};
use muster_herdr::daemon::Reached;
use muster_herdr::{
    HerdrBackend, HerdrClient, PaneEnvironment, configuration_text, own_socket_path, pinned, remote,
};
use muster_ssh::{Forward, Remote, Tunnel, remote_environment};
use serde_json::json;

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
    let tunnel = Tunnel::open(
        Forward {
            host: host.clone(),
            options,
            control_path: temporary
                .join("muster-devenv-install.ctl")
                .to_string_lossy()
                .into_owned(),
            local_socket: temporary
                .join("muster-devenv-install.sock")
                .to_string_lossy()
                .into_owned(),
            remote_socket: remote_socket.clone(),
        },
        std::sync::Arc::new(|_| {}),
    )
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

    // A shell that says it ran, named by Muster's own config and by nothing over there. What
    // it proves is the whole of a_29BYC0IGC: a setting written once reaches the panes on the
    // devenv as well as the ones beside them. It ends in /bin/sh because a pane whose program
    // exits takes the pane, then the workspace, then the daemon with it.
    let marker = "/tmp/muster-devenv-shell.ran";
    far.shell(&format!("rm -f {}", muster_ssh::quoted(marker)))
        .expect("the far end should let its own scratch be cleared");
    far.place(
        "/tmp/muster-devenv-shell",
        format!("#!/bin/sh\ntouch {marker}\nexec /bin/sh \"$@\"\n").as_bytes(),
        "0755",
    )
    .expect("the marker shell should arrive");
    let asked = config::parse(
        "scrollback_bytes = 4096\n\n[shell]\ncommand = \"/tmp/muster-devenv-shell\"\nmode = \"non_login\"\n",
    )
    .expect("this is a config file Muster accepts");

    let started = remote::ensure_running(
        &far,
        &environment,
        tunnel.local_socket_path(),
        Some(&cache()),
        &configuration_text(&asked.panes),
    )
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

    a_pane_runs_what_musters_config_named(&far, tunnel.local_socket_path(), &environment, marker);

    // Asked a second time, a daemon that is already holding somebody's agents is reused. That
    // is "sessions outlive the app" at one machine's remove, and it is also what keeps a second
    // launch from paying for the install again.
    let adopted = remote::ensure_running(
        &far,
        &environment,
        tunnel.local_socket_path(),
        Some(&cache()),
        &configuration_text(&asked.panes),
    )
    .expect("the second attach should find the daemon it started");
    assert_eq!(adopted, Reached::Adopted, "the daemon was already answering");

    // Left as it was found, so the corpus probe that runs after this starts its own daemon
    // rather than meeting one nobody expected.
    far.shell("pkill -x herdr >/dev/null 2>&1; rm -rf \"$HOME/.muster\"; true")
        .expect("the far end should let its own home be cleared");
}

/// The two halves of "a devenv pane runs the settings a laptop pane does".
///
/// A pane made through the real request path rather than a hand-built `workspace.create`, for
/// the reason `daemon_settings.rs` gives about the local case: the environment a pane is handed
/// is built in that path, and a test that called the daemon directly would prove the daemon's
/// half and quietly skip Muster's.
fn a_pane_runs_what_musters_config_named(
    far: &Remote,
    socket_path: &str,
    environment: &std::collections::BTreeMap<String, String>,
    marker: &str,
) {
    let backend = HerdrBackend::new(
        HerdrClient::new(socket_path),
        PaneEnvironment::restoring(environment),
        Names::alone("devenv", Mint::Backend),
    );
    backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a daemon that answered a snapshot can make a workspace");

    let client = HerdrClient::new(socket_path);
    let pane = until_some("the daemon to describe the pane it just made", || {
        let answer = client.request("pane.list", &json!({})).ok()?;
        let panes = answer.get("payload").unwrap_or(&answer).get("panes")?.as_array()?;
        Some(panes.first()?.get("pane_id")?.as_str()?.to_string())
    });

    // The shell Muster's config named ran, so the daemon over there read Muster's file rather
    // than whatever herdr's own rules would have found.
    let ran = until_some("the pane's shell to record that it ran", || {
        far.shell(&format!("test -e {} && echo yes || echo no", muster_ssh::quoted(marker)))
            .ok()
            .filter(|said| said.trim() == "yes")
    });
    assert_eq!(ran.trim(), "yes");

    // And the pane was handed the far machine's own herdr config back, so `herdr` typed in a
    // devenv pane reads what it always did rather than the file Muster derived for its daemon.
    let dump = "/tmp/muster-devenv-env.txt";
    client
        .request("pane.send_text", &json!({ "pane_id": pane, "text": format!("env > {dump}\n") }))
        .expect("a pane accepts text");
    let written = until_some("the shell in the pane to write its environment out", || {
        far.shell(&format!("cat {} 2>/dev/null", muster_ssh::quoted(dump)))
            .ok()
            .filter(|text| text.contains("PATH="))
    });
    // Asked of the same function that built what the pane was handed, so this pins that the
    // pane got the far machine's own file rather than pinning a second copy of the rule.
    let theirs = muster_herdr::discovery::config_file(environment)
        .expect("the container's environment says where its herdr config would be");
    assert!(
        written.lines().any(|line| line == format!("HERDR_CONFIG_PATH={theirs}")),
        "a devenv pane was not handed that machine's own herdr config back, and holds: {:?}",
        written.lines().filter(|line| line.starts_with("HERDR_")).collect::<Vec<&str>>()
    );

    let _ = far.shell(&format!("rm -f {}", muster_ssh::quoted(dump)));
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
