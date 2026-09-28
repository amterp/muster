//! A tab this window holds on a daemon still being started stays this window's.
//!
//! The record of who holds each tab forgets a tab no daemon describes, unless the window holding
//! it follows a daemon that has not answered, which may be where the tab is. A daemon still on
//! its way is one this window will show, so it counts as followed; were it not, this window
//! would give up its own tabs on a slow machine the moment it opened, leaving them to whichever
//! window came to the front next.
//!
//! What stages "on its way" is a daemon Muster starts, whose program never answers: the window
//! opens while the launch is still waiting. A daemon at a socket somebody named cannot stage it,
//! because it is followed as soon as an attempt begins, answering or not.
//!
//! Its own binary because it points `MUSTER_HOME` at a scratch home before anything reads it,
//! so the daemon Muster starts is this test's rather than the developer's.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, WindowName};
use muster_core::mirror::backend::TabId;
use muster_harness::DAEMON_DATA;
use prost::Message;

#[test]
fn a_tab_on_a_daemon_still_starting_stays_this_windows() {
    let home = scratch_home();
    // SAFETY: no other thread exists yet. Nothing in this binary has been started, and the seam
    // reads the environment on the first request rather than at load.
    unsafe {
        std::env::set_var("MUSTER_HOME", &home);
    }
    // A daemon that never answers, so the launch waits for it well past the window opening. It
    // outlives the test by a few seconds at most.
    let silent = home.join("silent-daemon");
    std::fs::write(&silent, "#!/bin/sh\nexec sleep 15\n").expect("the scratch home is writable");
    std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755))
        .expect("the script can be made executable");
    let config = home.join("config.toml");
    std::fs::write(&config, "[[daemon]]\nid = \"local\"\n").expect("the config can be written");

    // The record as a launch left it: this window, closed, holding a tab on that daemon.
    let arrangement = home.join("window-1.toml");
    let record = home.join("holding/tabs.toml");
    let mut holders = Holders::default();
    holders.opened(HeldWindow {
        name: WindowName::new("window-1"),
        arrangement: arrangement.to_string_lossy().into_owned(),
        socket: String::new(),
        pid: 1,
        focused: 0,
        daemons: std::iter::once(DaemonId::new("local")).collect(),
    });
    holders.take(TabId::new("t-left"), &WindowName::new("window-1"));
    holders.closed(&WindowName::new("window-1"));
    std::fs::create_dir_all(record.parent().expect("the record is in a directory"))
        .expect("the record's directory can be made");
    std::fs::write(&record, to_toml(&holders)).expect("the record can be written");

    let _turn = muster::testing::fresh_session();
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: silent.to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        config_path: config.to_string_lossy().into_owned(),
        state_path: arrangement.to_string_lossy().into_owned(),
        tab_holders_path: record.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    let now = from_toml(&std::fs::read_to_string(&record).unwrap_or_default())
        .expect("the record this window writes reads back");
    let held: Vec<String> =
        now.held_by(&WindowName::new("window-1")).map(ToString::to_string).collect();
    assert_eq!(held, ["t-left"], "the window let go of its tab on a daemon still starting");
}

/// A home this test owns, so nothing here can resolve to a real one.
fn scratch_home() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/holding-starting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
