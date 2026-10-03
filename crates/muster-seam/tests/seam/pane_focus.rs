//! A program that asked to hear focus (mode 1004) hears it as a Ghostty surface would tell it.
//!
//! The daemon writes the report and only to a program that asked; that is its own suite's to
//! pin. What needs a window is the other half: that the window says so when the pane with its
//! keyboard gains or loses focus, and not otherwise.

use std::path::Path;

use muster::proto::{OpenWindow, Request, Response, Startup, WindowFocus, request, response};
use muster_daemon_proto::pane_request;
use muster_harness::requests::{create, in_new_tab, make, until_text};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn the_pane_with_the_keyboard_hears_its_window_gain_and_lose_focus() {
    let _turn = muster::testing::fresh_session();
    let (_daemon, heard) = a_window_focused_on_a_program_that_asked();
    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: false })));
    holds(&heard, b"\x1b[I\x1b[O");
}

/// A report sent while the window has no connection to the daemon is dropped, so when the
/// daemon comes back each of its panes is told again where it stands. A handoff is the
/// reconnect a test can cause, and the program in the pane lives through it.
#[test]
fn a_daemon_that_comes_back_tells_the_pane_again_that_it_has_focus() {
    let _turn = muster::testing::fresh_session();
    let (mut daemon, heard) = a_window_focused_on_a_program_that_asked();
    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), muster_daemon_proto::Outcome::Done, "{}", answer.reason);
    holds(&heard, b"\x1b[I\x1b[I");
}

/// A program in a pane that asked to hear focus, in a window that has the keyboard on it and
/// has just been focused. Returns where the program writes what it heard.
fn a_window_focused_on_a_program_that_asked() -> (Daemon, std::path::PathBuf) {
    let daemon = Daemon::start_built();
    let heard = daemon.root().join("heard");
    let mut control = daemon.connect();
    make(
        &mut control,
        pane_request::Create {
            command: Some(format!(
                "stty raw -echo; printf '\\033[?1004h'; echo ready; cat > {}",
                heard.display()
            )),
            ..create("p1", in_new_tab("t1"))
        },
    );
    until_text(&mut control, "p1", "ready");

    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow::default())));

    // A window starts unfocused, so opening onto the pane tells it nothing: the first report is
    // the focus this test gives it.
    assert_ok(&dispatch(request::Payload::WindowFocus(WindowFocus { focused: true })));
    holds(&heard, b"\x1b[I");
    (daemon, heard)
}

fn holds(path: &Path, expected: &[u8]) {
    until(
        &format!("the program to have heard {:?}", String::from_utf8_lossy(expected)),
        || std::fs::read(path).is_ok_and(|bytes| bytes == expected),
        || {
            format!(
                "it has heard {:?}",
                String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default())
            )
        },
    );
}

fn dispatch(payload: request::Payload) -> Response {
    let reply = muster::dispatch(&Request::new(payload).encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
