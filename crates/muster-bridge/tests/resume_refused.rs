//! A parked pane another client has taken meanwhile is left to it.
//!
//! A hidden pane's bridge lets go of its herdr client, so while it is hidden nothing holds the
//! terminal and a second window can attach to it without taking anything. Showing the pane here
//! again then asks for a terminal somebody else holds, and herdr refuses. Before parking, this
//! window held on and the second one's takeover won; the same has to be true after, or
//! switching back to a tab steals a pane another window is drawing - and the other window,
//! answering the same way, steals it back.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use muster::proto::{CreateTab, FocusTabRelative, request};
use support::{Typing, answer, assert_ok, restarts, until};

#[test]
fn a_pane_taken_while_parked_is_not_taken_back_when_shown() {
    let mut typing = Typing::start("");
    let pane = typing.pane.clone();
    typing.expect_on_screen("$", "the pane never painted a prompt, so it never attached");

    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the pane behind the new tab to let go of its herdr client",
        || typing.bridge.herdr_clients() == 0,
        || typing.bridge.diagnosis("the pane was never parked"),
    );

    // A second window's bridge, attaching the ordinary way: nothing holds the terminal, so it
    // needs no takeover.
    let mut other = Command::new(herdr_harness::binary())
        .args(["terminal", "session", "control", &pane])
        .args(["--cols", "80", "--rows", "24"])
        .env("HERDR_SOCKET_PATH", typing.daemon.socket_path())
        // Held open, because a client whose stdin reaches EOF releases the terminal.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the pinned herdr runs");
    std::thread::sleep(Duration::from_millis(500));

    assert_ok(&answer(request::Payload::FocusTabRelative(FocusTabRelative {
        direction: "previous".to_string(),
    })));
    until("the refused bridge to go", || typing.bridge.has_exited(), ());

    // A decision to take it back shows as a replacement being counted. Given time to be made.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && restarts(&pane) == Some(0) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        restarts(&pane),
        Some(0),
        "showing a parked pane took its terminal back from the window that attached to it \
         while it was hidden"
    );
    assert!(other.try_wait().is_ok_and(|exited| exited.is_none()), "the other window lost it");

    let _ = other.kill();
    let _ = other.wait();
}
