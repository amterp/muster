//! A daemon Muster starts that is still on its way: the window opens without waiting for it, and
//! the tabs this window holds on it stay this window's.
//!
//! What stages "on its way" is a daemon program that never answers, so the launch is still
//! waiting while the window opens. Its own binary because it points `MUSTER_HOME` at a scratch
//! home before anything reads it, so the daemon Muster starts is this test's rather than the
//! developer's.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use std::sync::Mutex;

use muster::proto::{
    Event, OpenWindow, ProblemsChanged, ReadWindow, Request, Response, Startup, event, request,
    response,
};
use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, WindowName};
use muster_core::mirror::backend::TabId;
use muster_harness::{DAEMON_DATA, PATIENCE, built_daemon, until};
use prost::Message;

/// The daemon Muster finds for itself, with no `[[daemon]]` configured, is started on a thread of
/// its own like a configured one. The first launch after an update can take most of a minute
/// while macOS checks the new binary, and a window that waited for it was a Muster that seemed
/// not to start.
#[test]
fn a_window_opens_while_the_daemon_it_found_is_still_starting() {
    let home = scratch_home();
    let config = home.join("no-daemons.toml");
    std::fs::write(&config, "").expect("the config can be written");

    let _turn = muster::testing::fresh_session();
    let asked = Instant::now();
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: silent_daemon().to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        config_path: config.to_string_lossy().into_owned(),
        state_path: home.join("window-2.toml").to_string_lossy().into_owned(),
        tab_holders_path: home.join("holding/tabs-2.toml").to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let waited = asked.elapsed();
    assert!(
        waited < Duration::from_secs(5),
        "the window waited {waited:?} for a daemon that has not answered"
    );
}

/// Muster's own daemon arriving after the window opened fills it: the window asks it for a tab
/// on its first snapshot, as it asks any daemon that holds nothing.
#[test]
fn muster_s_own_daemon_arriving_late_still_fills_the_window() {
    let home = scratch_home();
    // Past the second the window waits, then the real daemon with whatever it was started with.
    let late =
        script("late-daemon", &format!("sleep 2\nexec '{}' \"$@\"", built_daemon().display()));
    let _turn = muster::testing::fresh_session();
    // After the turn, so it is dropped first: the next test's turn starts with it stopped.
    let _stopped = Stopped(muster_daemon_proto::install::socket_path(home));
    open_with_no_daemon_configured(&late, "late");
    until(
        "a tab from the daemon that arrived late",
        || !keyboard().1.is_empty(),
        || format!("the keyboard is at {:?}", keyboard()),
    );
}

/// Muster's own daemon failing to start is a problem the window says, and tries again from, as
/// a configured daemon's is. It used to refuse the window, which then rendered nothing at all.
#[test]
fn muster_s_own_daemon_that_cannot_start_is_a_problem_not_a_refused_window() {
    scratch_home();
    let failing = script("failing-daemon", "exit 1");

    let _turn = muster::testing::fresh_session();
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note_problems));
    open_with_no_daemon_configured(&failing, "failing");
    until(
        "the window to say its daemon could not start",
        || problems().iter().any(|key| key == "daemon:local"),
        || format!("the problems raised are {:?}", problems()),
    );
}

/// A daemon binary that is not there will not be there on the next attempt either, so the window
/// says so once, as an error somebody has to act on, and stops trying (kan a_2YAdjHjmh). A daemon
/// that exits, as above, may start next time and is still tried again.
#[test]
fn a_daemon_binary_that_is_not_there_is_an_error_and_not_retried() {
    let home = scratch_home();
    let missing = home.join("no-such-daemon");
    let _turn = muster::testing::fresh_session();
    *PROBLEMS.lock().expect("a panicking test poisoned the problems") = None;
    muster::ffi::muster_set_event_callback(Some(note_problems));
    open_with_no_daemon_configured(&missing, "missing");
    let said = || {
        PROBLEMS
            .lock()
            .expect("a panicking test poisoned the problems")
            .clone()
            .and_then(|changed| changed.problems.into_iter().find(|p| p.key == "daemon:local"))
    };
    until(
        "the window to say its daemon is not there",
        || said().is_some(),
        || format!("the problems raised are {:?}", problems()),
    );
    let problem = said().expect("just waited for it");
    assert_eq!(problem.severity, "error", "nothing changes by waiting: {problem:?}");
    assert!(
        problem.detail.contains(&missing.display().to_string())
            && problem.detail.contains("stopped trying"),
        "the problem names the missing binary and says Muster stopped: {}",
        problem.detail
    );
}

/// The record of who holds each tab forgets a tab no daemon describes, unless the window holding
/// it follows a daemon that has not answered, which may be where the tab is. A daemon still on
/// its way is one this window will show, so it counts as followed; were it not, this window
/// would give up its own tabs on a slow machine the moment it opened, leaving them to whichever
/// window came to the front next.
///
/// A daemon at a socket somebody named cannot stage this, because it is followed as soon as an
/// attempt begins, answering or not; one Muster starts is not followed until it answers.
#[test]
fn a_tab_on_a_daemon_still_starting_stays_this_windows() {
    let home = scratch_home();
    let silent = silent_daemon();
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
        install: String::new(),
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
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));

    let now = from_toml(&std::fs::read_to_string(&record).unwrap_or_default())
        .expect("the record this window writes reads back");
    let held: Vec<String> =
        now.held_by(&WindowName::new("window-1")).map(ToString::to_string).collect();
    assert_eq!(held, ["t-left"], "the window let go of its tab on a daemon still starting");
}

/// A home these tests own, so nothing here can resolve to a real one.
fn scratch_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let path =
            PathBuf::from(format!("/tmp/muster-test/daemon-starting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the harness root should be writable");
        // SAFETY: the first thing every test here does, so no thread of this binary has read the
        // environment yet; a test arriving meanwhile waits in `get_or_init`. The seam reads it on
        // the first request rather than at load.
        unsafe {
            std::env::set_var("MUSTER_HOME", &path);
        }
        let silent = path.join("silent-daemon");
        std::fs::write(&silent, "#!/bin/sh\nexec sleep 15\n").expect("the home is writable");
        std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755))
            .expect("the script can be made executable");
        path
    })
}

/// Starts and opens a window with no `[[daemon]]` configured, so Muster starts `program` as its
/// own daemon. `name` keeps each test's window and record apart.
fn open_with_no_daemon_configured(program: &Path, name: &str) {
    let home = scratch_home();
    let config = home.join("no-daemons.toml");
    std::fs::write(&config, "").expect("the config can be written");
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: program.to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        config_path: config.to_string_lossy().into_owned(),
        state_path: home.join(format!("window-{name}.toml")).to_string_lossy().into_owned(),
        tab_holders_path: home
            .join(format!("holding/tabs-{name}.toml"))
            .to_string_lossy()
            .into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
}

/// An executable shell script in the scratch home.
fn script(name: &str, body: &str) -> PathBuf {
    let path = scratch_home().join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the home is writable");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the script can be made executable");
    path
}

/// Stops the daemon at this socket when dropped, so a daemon Muster started for a test does not
/// outlive it and answer the next one.
struct Stopped(PathBuf);

impl Drop for Stopped {
    fn drop(&mut self) {
        let _ = muster_daemon_client::launch::stop(&self.0, PATIENCE);
    }
}

/// The tab on screen and the pane with the keyboard.
fn keyboard() -> (String, String) {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return (String::new(), String::new());
    };
    let view = window.view.unwrap_or_default();
    let pane = view
        .regions
        .iter()
        .find(|region| region.region_id == view.focused_region)
        .map(|region| region.pane_id.clone())
        .unwrap_or_default();
    (view.tab_id, pane)
}

static PROBLEMS: Mutex<Option<ProblemsChanged>> = Mutex::new(None);

extern "C" fn note_problems(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which is
    // the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ProblemsChanged(problems)) = event.payload {
        *PROBLEMS.lock().expect("a panicking test poisoned the problems") = Some(problems);
    }
}

/// The key of each problem the window has raised.
fn problems() -> Vec<String> {
    PROBLEMS
        .lock()
        .expect("a panicking test poisoned the problems")
        .clone()
        .map(|changed| changed.problems.into_iter().map(|problem| problem.key).collect())
        .unwrap_or_default()
}

/// A daemon program that never answers, so a launch waits for it well past the window opening.
/// It outlives each test by a few seconds at most.
fn silent_daemon() -> PathBuf {
    scratch_home().join("silent-daemon")
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
