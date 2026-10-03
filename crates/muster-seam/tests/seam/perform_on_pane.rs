//! Ghostty's clear_screen, asked of the window and carried out by the pane's daemon.
//!
//! What the daemon does with it is its own suite's to pin. What needs a window is the route: the
//! request reaches the pane with the keyboard, carrying the key that asked, so a program on the
//! alternate screen - where Ghostty leaves the key to the program - still gets it.

use std::path::Path;

use muster::proto::{
    KeyEvent, OpenWindow, PerformOnPane, Request, Response, Startup, perform_on_pane, request,
    response,
};
use muster_daemon_proto::pane_request;
use muster_harness::requests::{create, in_new_tab, make, until_text};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn clear_screen_on_the_alternate_screen_hands_the_program_its_key() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let heard = daemon.root().join("heard");
    let mut control = daemon.connect();
    // The alternate screen, with the kitty protocol's key reporting, under which cmd+k is a
    // sequence rather than nothing.
    make(
        &mut control,
        pane_request::Create {
            command: Some(format!(
                "printf '\\033[?1049h\\033[>1uvim'; stty raw -echo min 1 time 0; \
                 dd bs=64 count=1 of={} 2>/dev/null; sleep 30",
                heard.display()
            )),
            ..create("p1", in_new_tab("t1"))
        },
    );
    until_text(&mut control, "p1", "vim");

    assert_ok(&dispatch(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&dispatch(request::Payload::OpenWindow(OpenWindow::default())));
    until("the window to show the pane", the_window_shows_the_pane, String::new);

    assert_ok(&dispatch(request::Payload::PerformOnPane(PerformOnPane {
        action: Some(perform_on_pane::Action::Clear(perform_on_pane::ClearScreen {})),
        key: Some(KeyEvent {
            action: "press".to_string(),
            key: "KeyK".to_string(),
            modifiers: vec!["super".to_string()],
            unshifted_codepoint: Some(u32::from('k')),
            ..KeyEvent::default()
        }),
    })));

    holds(&heard, b"\x1b[107;9u");
}

/// Whether the window has the pane yet, which is where its keyboard lands: a clear_screen before
/// then is refused, since no pane has the keyboard.
fn the_window_shows_the_pane() -> bool {
    matches!(
        dispatch(request::Payload::ReadWindow(muster::proto::ReadWindow {})).payload,
        Some(response::Payload::Window(window)) if window.panes.iter().any(|pane| pane.pane_id == "p1")
    )
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
