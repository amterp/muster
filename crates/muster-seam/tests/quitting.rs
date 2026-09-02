//! Which of two things quitting does, against a real daemon.
//!
//! There has only ever been one behaviour and it was never said anywhere: quitting leaves every
//! daemon running, forever, which is the founding promise and stays the default. What was
//! missing is that somebody finished for the day had no way to say so, and the only route to
//! ending a daemon was `pgrep` and a signal - which cost a working agent once, because the
//! process holding it looked exactly like the scratch daemons beside it (kan a_28YghIUw2).
//!
//! Both halves need a real daemon, because the thing being asserted is whether a process is
//! still there afterwards.

use herdr_harness::{Daemon, until};
use muster::proto::{OpenWindow, Quitting, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::json;

#[test]
fn quitting_leaves_the_session_running() {
    // The promise, pinned. Everything else in this file is the exception to it, and an
    // exception nothing guards the rule against is a rule that erodes.
    let _turn = muster::testing::fresh_session();
    let daemon = open_a_window();

    assert_ok(&answer(request::Payload::Quitting(Quitting { close_sessions: false })));

    assert!(
        daemon.socket_path().exists(),
        "quitting ended the daemon. Sessions outliving the app is why Muster puts its daemon in \
         a process group of its own - quitting must cost a session nothing at all"
    );
    assert!(
        answers(&daemon),
        "the daemon stopped answering after an ordinary quit, so whatever is in its panes is \
         gone and every promise about coming back to them is broken"
    );
}

#[test]
fn quitting_and_closing_sessions_ends_the_daemon() {
    // The other half, and the reason it can be offered at all: `server.stop` is a clean stop,
    // measured - a pane's process gets a catchable SIGHUP and a moment to act, not a SIGKILL.
    let _turn = muster::testing::fresh_session();
    let daemon = open_a_window();
    assert!(answers(&daemon), "the daemon should be answering before it is asked to stop");

    assert_ok(&answer(request::Payload::Quitting(Quitting { close_sessions: true })));

    // Waited for rather than asserted outright: the request is answered when the daemon
    // acknowledged the stop, and a process tearing down its panes takes a moment more.
    until(
        "the daemon to be gone",
        || !answers(&daemon),
        || {
            "the daemon is still answering after a quit that was asked to end it, so somebody who \
         said they were finished has a session still running and no sign that anything \
         happened"
                .to_string()
        },
    );
}

/// A daemon with a window open onto it, which is what makes it one this window would end.
fn open_a_window() -> Daemon {
    // Nothing here paints a pane, so none becomes typeable - an error, which opens the roster
    // and republishes. Noise rather than a finding, so it is switched off.
    // SAFETY: nothing else in this process reads the environment concurrently; this runs before
    // the daemon starts and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "quitting", "focus": true }));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    daemon
}

/// Whether anything is behind the socket, rather than whether the file is there.
///
/// A socket path outlives the daemon that bound it, so its presence answers nothing - which is
/// the same reason `daemon::answers` pings rather than looking.
fn answers(daemon: &Daemon) -> bool {
    let socket = daemon.socket_path().to_string_lossy().into_owned();
    std::os::unix::net::UnixStream::connect(&socket).is_ok()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
