//! A daemon an older Muster left running is handed to this build's daemon, agents and all.
//!
//! A newer app used to adopt it as it was, so a daemon fix reached a machine only when that
//! daemon restarted, which ends every agent in it (MIP-3, section 10). The older daemon here is
//! today's saying an older version (`MUSTER_DAEMON_VERSION_SAID`, read only by a debug build),
//! found where this install's daemon listens, which is the only socket Muster hands over.
//!
//! Its own binary because it points `MUSTER_HOME` at a scratch home before anything reads it, so
//! the socket Muster manages is this test's rather than the developer's.

use std::path::PathBuf;

use muster::proto::{OpenWindow, Request, Response, Startup, request, response};
use muster_daemon_proto::install;
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{DAEMON_DATA, Daemon, built_daemon, until_some};
use prost::Message;

#[test]
fn an_older_daemon_left_running_is_handed_to_this_builds() {
    let home = scratch_home();
    // SAFETY: no other thread exists yet. Nothing in this binary has been started, and the seam
    // reads the environment on the first request rather than at load.
    unsafe {
        std::env::set_var("MUSTER_HOME", &home);
    }
    let mut daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    // Where this install's daemon listens under that home, which is where Muster looks for one
    // left running.
    let managed = install::socket_path(&home);
    std::fs::create_dir_all(managed.parent().expect("the socket is in a directory"))
        .expect("the scratch home is writable");
    std::os::unix::fs::symlink(daemon.socket_path(), &managed).expect("the socket can be linked");
    let before = daemon.connect().welcome().instance;

    let _turn = muster::testing::fresh_session();
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: built_daemon().to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    let after = until_some("the older daemon to hand its panes to this build's", || {
        let welcome = daemon.connect().welcome().clone();
        (welcome.instance != before).then_some(welcome)
    });
    daemon.served_by(after.pid.cast_signed());
    let panes: Vec<String> =
        snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect();
    assert_eq!(panes, ["p1"], "the pane did not come through the handoff");
}

/// A home this test owns, so nothing here can resolve to a real one.
fn scratch_home() -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/older-daemon-{}", std::process::id()));
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
