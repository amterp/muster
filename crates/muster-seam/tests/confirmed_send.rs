//! Whether `pane send --confirm` actually catches a message the pane never showed.
//!
//! Without the flag a send answers the same way whether the program in the pane received a word
//! of it: the daemon takes the request, the request succeeded, and that is all anybody is told.
//! A pane whose terminal is in canonical mode drops a line over 1024 bytes whole and says
//! nothing (`observations/herdr-0.8.0.md` section 25), and a harness that folds a long paste
//! into a placeholder draws nothing either - the sender sees exit 0 for both.
//!
//! Staged against a program that takes the beginning of what it is handed and drops the rest,
//! which is what the failure looked like from the outside when it was found (kan a_2ImLVumEP).
//! The two sends differ only in length, so nothing but the length can explain the two answers.
//!
//! In the seam rather than in the CLI because that is where the check lives: the CLI holds no
//! logic, and a second caller wanting the same certainty - a chord, an API client - has to get
//! the same answer.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use herdr_harness::{Daemon, until};
use muster::proto::{
    OpenWindow, ReadPane, Request, Response, SendToPane, Startup, request, response,
};
use prost::Message;
use serde_json::json;

/// How much of a message the fixture draws before it starts dropping.
///
/// Short enough that a one-word send fits inside it and a sentence does not, so the two cases
/// below are one fixture and two lengths.
const DRAWS: usize = 20;

/// What the fixture prints once it has taken the terminal and is reading.
const READING: &str = "fixture-is-reading";

#[test]
fn a_send_the_pane_never_showed_is_refused_rather_than_reported_as_done() {
    let _turn = muster::testing::fresh_session();
    let drawing = scratch("confirmed-send");
    let daemon = Daemon::start_running(&fixture(&drawing).to_string_lossy());
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "confirmed", "focus": true }));

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    let pane = the_only_pane();
    wait_until_reading(&pane);

    // Short enough to be drawn whole, so the pane shows it and the check finds it. Without this
    // half, a `--confirm` that refused everything would pass the half below.
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane.clone(),
        text: "hello".to_string(),
        confirm: true,
        ..SendToPane::default()
    })));

    // And the same send, longer than the fixture will draw. The message reaches the pane in
    // full - nothing here truncates it - and the program shows the first of it, which is
    // exactly what a caller cannot tell from success.
    let refusal = refused(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane.clone(),
        text: "please read AGENTS.md end to end before you touch anything at all, and say what \
               you found before you change any of it"
            .to_string(),
        confirm: true,
        ..SendToPane::default()
    })));
    assert!(
        refusal.contains(&pane),
        "the refusal has to name the pane, since a caller driving four agents cannot act on \
         one that does not. It said: {refusal}"
    );

    // The same message without the flag is a success, which is the behaviour every caller that
    // did not ask for a round trip still gets.
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: "please read AGENTS.md end to end before you touch anything at all, and say what \
               you found before you change any of it"
            .to_string(),
        ..SendToPane::default()
    })));

    let _ = std::fs::remove_dir_all(&drawing);
}

/// The window's only pane, by Muster's name for it.
fn the_only_pane() -> String {
    let mut found = None;
    until(
        "the window to hold the daemon's pane",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(muster::proto::ReadWindow {})).payload
            else {
                return false;
            };
            found = window.panes.first().map(|pane| pane.pane_id.clone());
            found.is_some()
        },
        || "the window never listed a pane, so there was nothing to send to".to_string(),
    );
    found.expect("the wait above returns only once there is one")
}

/// Waits for the fixture's prompt, read back the way a caller would read it.
///
/// Through `ReadPane` rather than through the daemon, because what has to be true is that the
/// pane's text is reachable from here - which is the same route `--confirm` takes, and a
/// fixture whose output never arrived would otherwise look like a `--confirm` that does not
/// work.
fn wait_until_reading(pane: &str) {
    until(
        "the fixture to say it has taken the terminal",
        || read(pane).contains(READING),
        || format!("the pane shows {:?}", read(pane)),
    );
}

fn read(pane: &str) -> String {
    match answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
    .payload
    {
        Some(response::Payload::PaneText(text)) => text.text,
        _ => String::new(),
    }
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

fn refused(response: &Response) -> String {
    match &response.payload {
        Some(response::Payload::Failure(failure)) => failure.reason.clone(),
        other => panic!(
            "a send the pane never showed answered {other:?} rather than refusing.\n  Impact: \
             `--confirm` reports success it has not got, which is the whole thing it exists to \
             stop - a caller instructing an agent has no way left to tell a message that landed \
             from one that did not."
        ),
    }
}

/// A directory this test owns, beside the daemon roots rather than inside one.
fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}

/// A program that draws the first of what it is handed and silently drops the rest.
///
/// Not a caricature: a harness that folds a long paste into `[Pasted text #2]` and a terminal
/// that discards an over-long line both look exactly like this from outside, and both answered
/// exit 0 before this flag existed.
///
/// It must not exit - herdr closes a pane whose process ends, then the workspace, then the
/// daemon - so it loops rather than returning.
fn fixture(drawing: &Path) -> PathBuf {
    let script = drawing.join("draws-the-first-of-it.py");
    std::fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env python3
import os, select, tty

# Raw and echoless, so what appears on this pane is only what this program chose to draw.
tty.setraw(0)
os.write(1, "{READING}".encode() + b"\r\n")

while True:
    if select.select([0], [], [], 0.2)[0]:
        try:
            chunk = os.read(0, 65536)
        except OSError:
            continue
        if not chunk:
            continue
        # A paste arrives fenced; the fence is not the message and drawing it would put
        # escape bytes on the screen for a reader to trip over.
        text = chunk.replace(b"\x1b[200~", b"").replace(b"\x1b[201~", b"")
        os.write(1, text[:{DRAWS}] + b"\r\n")
"#
        ),
    )
    .expect("the scratch directory should be writable");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the fixture should be executable");
    script
}
