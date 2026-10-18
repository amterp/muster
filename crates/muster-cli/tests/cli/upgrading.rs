//! `muster upgrade` from a pane: the real binary, through a real window, runs the line the app
//! worked out at launch in a tab on the daemon here, with a fake `brew` standing in for Homebrew.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn an_upgrade_from_a_pane_runs_brew_in_a_tab_and_prints_it() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let brew = fake_brew(daemon.root());
    let ran = daemon.root().join("brew.ran");
    let socket = open_window(
        &daemon,
        &format!("'{}' upgrade --cask 'amterp/tap/muster'", brew.display()),
        "",
    );

    let (code, out, errors) = muster(&["upgrade"], &socket);
    assert_eq!(code, 0, "`muster upgrade` failed: {errors}");
    let pane = out.trim();
    assert!(
        !pane.is_empty() && pane.bytes().all(|byte| byte.is_ascii_lowercase()),
        "`muster upgrade` printed {out:?} rather than its pane's callsign"
    );
    until(
        "the fake brew to run",
        || std::fs::read_to_string(&ran).is_ok_and(|text| !text.is_empty()),
        || format!("{} was never written", ran.display()),
    );
    assert_eq!(std::fs::read_to_string(&ran).unwrap(), "upgrade --cask amterp/tap/muster\n");
}

#[test]
fn an_upgrade_the_app_cannot_do_is_refused_with_its_reason() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let why = "Muster was not upgraded: Homebrew did not install this one.";
    let socket = open_window(&daemon, "", why);

    let (code, _, errors) = muster(&["upgrade"], &socket);
    assert_eq!(code, 1, "a refusal exits 1, got {code}: {errors}");
    assert!(errors.contains(why), "the refusal lost the app's reason: {errors}");
}

/// Starts the window as the app would, with its upgrade line or its refusal, and returns the
/// socket a pane would be handed.
fn open_window(daemon: &Daemon, command: &str, refused: &str) -> String {
    let socket = daemon.root().join("command.sock").to_string_lossy().into_owned();
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            command_socket_path: socket.clone(),
            upgrade_command: command.to_string(),
            upgrade_refused: refused.to_string(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        let bytes = Request::new(payload).encode_to_vec();
        let response = Response::decode(muster::dispatch(&bytes).as_slice())
            .expect("the core answers with a response this build knows");
        if let Some(response::Payload::Failure(failure)) = &response.payload {
            panic!("the core refused: {}", failure.reason);
        }
    }
    socket
}

/// A `brew` that writes down what it was asked and does nothing else.
fn fake_brew(directory: &Path) -> std::path::PathBuf {
    let brew = directory.join("brew");
    let script = format!("#!/bin/sh\necho \"$@\" >> '{}/brew.ran'\n", directory.display());
    std::fs::write(&brew, script).expect("the scratch directory is writable");
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755))
        .expect("the fake brew can be made executable");
    brew
}

/// The real binary, with only the socket a pane would have: an inherited `MUSTER_SOCKET` would
/// point it at the developer's own window.
fn muster(argv: &[&str], socket: &str) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_muster"))
        .args(argv)
        .env_clear()
        .env("MUSTER_SOCKET", socket)
        .output()
        .unwrap_or_else(|error| panic!("the muster binary could not be run: {error}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}
