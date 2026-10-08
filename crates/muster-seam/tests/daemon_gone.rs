//! A daemon Muster started that goes away is started again, and its panes come back.
//!
//! A devenv rebooted and its daemon with it, and the window went on retrying a socket nothing
//! would answer again, so its panes stayed as they were for two days, until a relaunch started
//! the daemon (kan a_2cJ6cgiLw). Its own binary because it points `MUSTER_HOME` at a scratch
//! home before anything reads it, so the daemon Muster starts is this test's rather than the
//! developer's; what stages the reboot is killing that daemon outright.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use muster::proto::{OpenWindow, ReadWindow, Request, Response, Startup, request, response};
use muster_harness::{Control, DAEMON_DATA, PATIENCE, built_daemon, until};
use prost::Message;

#[test]
fn a_daemon_muster_started_that_dies_is_started_again_with_its_panes() {
    let home = scratch_home();
    let socket = muster_daemon_proto::install::socket_path(home);
    let _turn = muster::testing::fresh_session();
    // After the turn, so it is dropped first: nothing is left running for the next test.
    let _stopped = Stopped(socket.clone());
    // No bridge runs here, and a pane waiting for one would raise a problem of its own.
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    open_with_no_daemon_configured(home);
    until(
        "the window to open a pane on the daemon it started",
        || first_pane().is_some(),
        || format!("the window reads {:?}", read_window()),
    );
    let pane = first_pane().expect("just waited for it");
    // Only what the daemon wrote down comes back, so the kill waits for the pane to be in its file.
    let saved = socket.with_extension("state.json");
    until(
        "the daemon to write the pane down",
        || std::fs::read_to_string(&saved).is_ok_and(|state| state.contains(&pane)),
        || std::fs::read_to_string(&saved).unwrap_or_default(),
    );
    let before = Control::connect(&socket).welcome().clone();

    // SAFETY: a signal to a process this test's window started, by the pid it gave.
    let killed =
        unsafe { libc::kill(i32::try_from(before.pid).expect("a pid fits"), libc::SIGKILL) };
    assert_eq!(killed, 0, "the daemon {} could not be killed", before.pid);

    until(
        "another run of the daemon to answer",
        || {
            Control::try_connect(&socket)
                .is_ok_and(|control| control.welcome().instance != before.instance)
        },
        || format!("the window reads {:?}", read_window().daemons),
    );
    until(
        "the window to follow it again, as a daemon it started, with the pane back",
        || {
            let window = read_window();
            let machine = window.daemons.first().cloned().unwrap_or_default();
            machine.state == "connected"
                && machine.started_by_muster
                && window
                    .panes
                    .iter()
                    .any(|agent| agent.pane_id == pane && agent.state != "unreachable")
        },
        || format!("the window reads {:?}", read_window()),
    );
}

/// The first pane the window lists, by its name.
fn first_pane() -> Option<String> {
    read_window().panes.into_iter().next().map(|agent| agent.pane_id)
}

/// A home this test owns, so nothing here can resolve to a real one.
fn scratch_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let path = PathBuf::from(format!("/tmp/muster-test/daemon-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the harness root should be writable");
        // SAFETY: the first thing the test here does, so no thread of this binary has read the
        // environment yet. The seam reads it on the first request rather than at load.
        unsafe {
            std::env::set_var("MUSTER_HOME", &path);
        }
        path
    })
}

/// Starts and opens a window with no `[[daemon]]` configured, so Muster starts its own daemon.
fn open_with_no_daemon_configured(home: &Path) {
    let config = home.join("no-daemons.toml");
    std::fs::write(&config, "").expect("the config can be written");
    assert_ok(&answer(request::Payload::Startup(Startup {
        daemon_path: built_daemon().to_string_lossy().into_owned(),
        daemon_data_path: DAEMON_DATA.to_string(),
        config_path: config.to_string_lossy().into_owned(),
        state_path: home.join("window-gone.toml").to_string_lossy().into_owned(),
        tab_holders_path: home.join("holding/tabs-gone.toml").to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
}

/// Stops the daemon at this socket when dropped, whichever run of it is there.
struct Stopped(PathBuf);

impl Drop for Stopped {
    fn drop(&mut self) {
        let _ = muster_daemon_client::launch::stop(&self.0, PATIENCE);
    }
}

fn read_window() -> muster::proto::Window {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
