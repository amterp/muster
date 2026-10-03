//! A daemon an older Muster left running is handed to this build's daemon, agents and all.
//!
//! A newer app used to adopt it as it was, so a daemon fix reached a machine only when that
//! daemon restarted, which ends every agent in it (MIP-3, section 10). The older daemon here is
//! today's saying an older version (`MUSTER_DAEMON_VERSION_SAID`, read only by a debug build),
//! found where this install's daemon listens, which is the only socket Muster hands over.
//!
//! Its own binary because it points `MUSTER_HOME` at a scratch home before anything reads it, so
//! the socket Muster manages is these tests' rather than the developer's. The tests take the
//! session's turn before touching that socket, so they share it one at a time.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use muster::proto::{
    Event, OpenWindow, ProblemsChanged, Request, Response, Startup, event, request, response,
};
use muster_daemon_proto::install;
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{DAEMON_DATA, Daemon, built_daemon, until, until_some};
use prost::Message;

#[test]
fn an_older_daemon_left_running_is_handed_to_this_builds() {
    let home = scratch_home();
    let mut daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let before = daemon.connect().welcome().instance;

    let _turn = muster::testing::fresh_session();
    left_running(home, &daemon);
    open_a_window();

    let after = until_some("the older daemon to hand its panes to this build's", || {
        let welcome = daemon.connect().welcome().clone();
        (welcome.instance != before).then_some(welcome)
    });
    daemon.served_by(after.pid.cast_signed());
    let panes: Vec<String> =
        snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect();
    assert_eq!(panes, ["p1"], "the pane did not come through the handoff");
}

#[test]
fn a_refusal_is_said_once_and_the_older_daemon_goes_on() {
    let home = scratch_home();
    let daemon = Daemon::start_with(
        built_daemon(),
        &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1"), ("MUSTER_DAEMON_HANDOFF_FAULT", "refuse")],
    );
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let before = daemon.connect().welcome().instance;

    let _turn = muster::testing::fresh_session();
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note));
    left_running(home, &daemon);
    open_a_window();

    until(
        "the window to say the older daemon kept its panes",
        || problems().iter().any(|(key, _)| key == "handover:local"),
        || format!("the problems raised are {:?}", problems()),
    );
    let said: Vec<String> = problems()
        .into_iter()
        .filter(|(key, _)| key == "handover:local")
        .map(|(_, detail)| detail)
        .collect();
    assert_eq!(said.len(), 1, "the refusal was said more than once: {said:?}");
    assert!(said[0].contains("nothing in them is lost"), "the refusal said {said:?}");
    assert_eq!(daemon.connect().welcome().instance, before, "a refused handoff changed daemons");
}

/// A home these tests own, so nothing here can resolve to a real one, pointed at once for the
/// whole binary.
fn scratch_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let path = PathBuf::from(format!("/tmp/muster-test/older-daemon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the harness root should be writable");
        // SAFETY: the first thing every test here does, so no thread of this binary has read the
        // environment yet; a test arriving meanwhile waits in `get_or_init`. The seam reads it on
        // the first request rather than at load.
        unsafe {
            std::env::set_var("MUSTER_HOME", &path);
        }
        path
    })
}

/// Leaves `daemon` where this install's daemon listens, which is where Muster looks for one
/// left running. Replaces whatever the last test left there.
fn left_running(home: &Path, daemon: &Daemon) {
    let managed = install::socket_path(home);
    std::fs::create_dir_all(managed.parent().expect("the socket is in a directory"))
        .expect("the scratch home is writable");
    let _ = std::fs::remove_file(&managed);
    std::os::unix::fs::symlink(daemon.socket_path(), &managed).expect("the socket can be linked");
}

fn open_a_window() {
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: built_daemon().to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
}

static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ProblemsChanged(problems)) = event.payload {
        *PROBLEMS.lock().expect("a panicking test poisoned the problems") = Some(problems);
    }
}

/// Each problem the window has raised, by key.
fn problems() -> Vec<(String, String)> {
    PROBLEMS
        .lock()
        .expect("a panicking test poisoned the problems")
        .clone()
        .map(|changed| {
            changed.problems.into_iter().map(|problem| (problem.key, problem.detail)).collect()
        })
        .unwrap_or_default()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
