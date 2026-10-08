//! Upgrading the app: the line the shell handed over at launch runs in a new tab on this
//! machine's daemon, with the keyboard on it, and a shell that found nothing to upgrade with is
//! the refusal.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use muster::proto::{
    OpenWindow, ReadWindow, Request, Response, Startup, UpgradeMuster, request, response,
};
use muster_harness::requests::{create, in_new_tab, make, snapshot, until_text};
use muster_harness::{Daemon, until};
use prost::Message;

/// A fake `brew` stands in for Homebrew: the tab runs exactly what the shell handed over, and it
/// is a tab of its own on the daemon here, named, with the keyboard in it.
#[test]
fn an_upgrade_runs_the_shells_line_in_a_tab_of_its_own() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let scratch = scratch("upgrade");
    let brew = fake_brew(&scratch);
    let ran = scratch.join("brew.ran");
    let line = format!("'{}' upgrade --cask 'amterp/tap/muster'", brew.display());
    open_window(&daemon, &line, "");

    let made = match answer(request::Payload::UpgradeMuster(UpgradeMuster {})).payload {
        Some(response::Payload::Made(made)) => made,
        other => panic!("an upgrade answered {other:?}"),
    };
    let record = snapshot(&mut control).panes.into_iter().find(|pane| pane.pane == made.pane_id);
    let record = record.expect("the upgrade's pane is on the daemon here");
    assert_eq!(record.command.as_deref(), Some(line.as_str()));
    assert_eq!(record.label.as_deref(), Some("⬆️ Upgrade Muster"));
    assert_eq!(keyboard_pane(), made.pane_id, "the keyboard did not follow the upgrade");
    until(
        "the fake brew to run",
        || std::fs::read_to_string(&ran).is_ok_and(|text| !text.is_empty()),
        || format!("{} was never written", ran.display()),
    );
    assert_eq!(std::fs::read_to_string(&ran).unwrap(), "upgrade --cask amterp/tap/muster\n");
}

/// The shell's own sentence is the refusal, word for word, and nothing is made.
#[test]
fn an_upgrade_the_shell_refused_says_why_and_makes_nothing() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    let why = "Muster was not upgraded: Homebrew did not install this one.";
    open_window(&daemon, "", why);

    let refused = answer(request::Payload::UpgradeMuster(UpgradeMuster {}));
    assert_eq!(failure(&refused), why);
    assert_eq!(snapshot(&mut control).panes.len(), 1, "a pane was made anyway");
}

/// A shell that named no way at all, as any but the macOS one does, is refused too, rather
/// than running nothing in a new tab.
#[test]
fn an_upgrade_with_no_way_named_is_refused() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    open_window(&daemon, "", "");

    let refused = answer(request::Payload::UpgradeMuster(UpgradeMuster {}));
    assert!(failure(&refused).contains("named no way to upgrade itself"), "{refused:?}");
    assert_eq!(snapshot(&mut control).panes.len(), 1, "a pane was made anyway");
}

fn open_window(daemon: &Daemon, command: &str, refused: &str) {
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            upgrade_command: command.to_string(),
            upgrade_refused: refused.to_string(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        let response = answer(payload);
        if let Some(response::Payload::Failure(failure)) = &response.payload {
            panic!("the core refused: {}", failure.reason);
        }
    }
    until(
        "the window to show a tab",
        || !keyboard_pane().is_empty(),
        || "the window shows nothing".to_string(),
    );
}

/// A `brew` that writes down what it was asked and does nothing else.
fn fake_brew(directory: &Path) -> PathBuf {
    let brew = directory.join("brew");
    let script = format!("#!/bin/sh\necho \"$@\" >> '{}/brew.ran'\n", directory.display());
    std::fs::write(&brew, script).expect("the scratch directory is writable");
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755))
        .expect("the fake brew can be made executable");
    brew
}

fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the scratch directory can be made");
    path
}

/// The pane with the keyboard.
fn keyboard_pane() -> String {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return String::new();
    };
    let view = window.view.unwrap_or_default();
    view.regions
        .iter()
        .find(|region| region.region_id == view.focused_region)
        .map(|region| region.pane_id.clone())
        .unwrap_or_default()
}

fn failure(response: &Response) -> String {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => failure.reason.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}
