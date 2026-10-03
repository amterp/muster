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

use muster::proto::{
    OpenWindow, ReadPane, Request, Response, SendToPane, Startup, request, response,
};
use muster_daemon_proto::pane_request;
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// How much of a message the fixture draws before it starts dropping.
///
/// Short enough that a one-word send fits inside it and a sentence does not, so the two cases
/// below are one fixture and two lengths.
const DRAWS: usize = 20;

/// What the fixture prints once it has taken the terminal and is reading.
const READING: &str = "fixture-is-reading";

/// How long the slow fixture below sits on a message before drawing it.
///
/// Between the two numbers that matter, with room either side. The slowest an honest pane was
/// measured taking to draw what it was handed is 54ms, so a confirmation that reads once and
/// gives up fails this deterministically rather than occasionally; and it is well inside the
/// budget in `confirm_it_arrived`, so a confirmation that waits passes it with margin to spare
/// on a runner slower than the machine it was written on.
const DRAWS_AFTER: f32 = 0.3;

/// A confirmed send to a pane that draws what it is handed, only later than an echo would, is
/// confirmed: slow is not the same as deaf.
#[test]
fn a_send_the_pane_draws_late_is_confirmed_rather_than_refused() {
    let _turn = muster::testing::fresh_session();
    let drawing = scratch("confirmed-send-late");
    let daemon = daemon_running(&slow_fixture(&drawing, 0));

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let pane = the_only_pane();
    wait_until_reading(&pane);

    // The whole claim: a pane that is slow rather than deaf is confirmed. A send is accepted
    // once the daemon holds the bytes, which is before the program has read them, echoed them
    // and had that reach the daemon's copy of the screen - so a confirmation that reads at that
    // moment and gives up refuses a message that arrived, which is the one answer this flag
    // exists to make impossible.
    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: "hello".to_string(),
        confirm: true,
        ..SendToPane::default()
    })));

    let _ = std::fs::remove_dir_all(&drawing);
}

/// A confirmed send whose own output scrolls it far up the pane before anything reads it is still
/// confirmed. `seq 1 100000` sent to a shell does this: by the first read after the echo, the
/// echoed line is thousands of rows above the bottom, and a confirmation that only ever looks near
/// the bottom refuses a command that ran - inviting a caller to run it twice.
#[test]
fn a_send_whose_output_scrolls_it_away_is_still_confirmed() {
    let _turn = muster::testing::fresh_session();
    let drawing = scratch("confirmed-send-scrolled");
    let daemon = daemon_running(&slow_fixture(&drawing, 2000));

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let pane = the_only_pane();
    wait_until_reading(&pane);

    assert_ok(&answer(request::Payload::SendToPane(SendToPane {
        pane_id: pane,
        text: "run the thing".to_string(),
        confirm: true,
        ..SendToPane::default()
    })));

    let _ = std::fs::remove_dir_all(&drawing);
}

/// A confirmed send the pane drew only the start of is refused, naming the pane, while the same
/// send unconfirmed and a short one confirmed both succeed - so only the length explains it.
#[test]
fn a_send_the_pane_never_showed_is_refused_rather_than_reported_as_done() {
    let _turn = muster::testing::fresh_session();
    let drawing = scratch("confirmed-send");
    let daemon = daemon_running(&fixture(&drawing));

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
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

/// A send to a daemon the window has lost is refused rather than answered as sent. Nothing
/// was queued, so exit 0 would tell an agent its instruction landed when not a byte of it did
/// (kan a_2LOHfLmsL, where a sender believed a lost message and never resent it).
#[test]
fn a_send_to_a_daemon_that_has_gone_is_refused() {
    let _turn = muster::testing::fresh_session();
    let mut daemon = Daemon::start_built();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let pane = the_only_pane();
    daemon.kill();

    let last = std::cell::RefCell::new(None);
    until(
        "a send to the stopped daemon to be refused",
        || {
            let sent = answer(request::Payload::SendToPane(SendToPane {
                pane_id: pane.clone(),
                text: "hello".to_string(),
                ..SendToPane::default()
            }));
            let refused = matches!(sent.payload, Some(response::Payload::Failure(_)));
            *last.borrow_mut() = Some(sent);
            refused
        },
        || format!("the send kept answering {:?}", last.borrow()),
    );
}

/// A daemon holding one pane, running `program` in place of an interactive shell.
fn daemon_running(program: &Path) -> Daemon {
    let daemon = Daemon::start_built();
    make(
        &mut daemon.connect(),
        pane_request::Create {
            command: Some(program.to_string_lossy().into_owned()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    daemon
}

/// The window's only pane, by Muster's name for it.
fn the_only_pane() -> String {
    let mut found = None;
    until(
        "the window to hold the daemon's pane",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(muster::proto::ReadWindow::default())).payload
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
    let bytes = Request::new(payload).encode_to_vec();
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
/// It must not exit - the pane would drop back to its shell, and what the pane shows would
/// stop being this program's - so it loops rather than returning.
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

/// A program that draws everything it is handed, but not straight away, followed in the same write
/// by `lines_after` lines of output of its own.
///
/// The honest slow pane, which is what separates "did not receive it" from "has not drawn it
/// yet". A harness redrawing a composer around a long paste, and a pane whose daemon is at the
/// far end of an SSH forward, both take longer than an echo does. The lines after are a command
/// whose output lands before anything can read the echo above it.
fn slow_fixture(drawing: &Path, lines_after: usize) -> PathBuf {
    let script = drawing.join("draws-it-late.py");
    std::fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env python3
import os, select, time, tty

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
        text = chunk.replace(b"\x1b[200~", b"").replace(b"\x1b[201~", b"")
        time.sleep({DRAWS_AFTER})
        after = "".join(f"output line {{n}}\r\n" for n in range({lines_after})).encode()
        os.write(1, text + b"\r\n" + after)
"#
        ),
    )
    .expect("the scratch directory should be writable");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the fixture should be executable");
    script
}
