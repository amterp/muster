//! What a program in a pane actually receives when Muster is told to send it something.
//!
//! `backend-intent.json` pins the verb. What a corpus case cannot say is what that verb does
//! once a program is reading, and the two herdr offers differ in exactly one way that matters:
//! `pane.send_input` is encoded against the pane's live modes, so the daemon fences the text in
//! bracketed paste when the program asked to be told about a paste, and `pane.send_text` writes
//! the bytes as they are.
//!
//! The difference is invisible for a single line and decides the whole message for several.
//! Unfenced, every newline arrives as a submission: a harness reads three lines as three
//! prompts and acts on the last one. That was measured against a real agent before it was
//! measured here (kan a_2ImLVumEP, `observations/herdr-0.8.0.md` section 25), and this is the
//! assertion that keeps it fixed.
//!
//! A fixture rather than a shell, because a shell is the one receiver that hides the bug: it
//! runs a line editor which reads a paste and a newline the same way round for a single line,
//! and macOS's `/bin/sh` predates bracketed paste entirely.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use herdr_harness::{Daemon, until_some};
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::names::{Mint, Names};
use muster_herdr::{HerdrBackend, HerdrClient, PaneEnvironment};
use serde_json::json;

/// What the fixture prints once it has taken the terminal and is reading.
const READING: &str = "fixture-is-reading";

/// Three lines sent as one message arrive as one paste, not as three submissions.
#[test]
fn a_multi_line_message_arrives_fenced_as_a_single_paste() {
    let received = scratch("pane-sending");
    let daemon = Daemon::start_running(&fixture(&received).to_string_lossy());
    let names = Names::alone("local", Mint::Drawn);
    let backend = HerdrBackend::new(daemon.client(), PaneEnvironment::none(), names);

    let pane = backend
        .submit(&BackendIntent::CreateWorkspace {
            cwd: Some("/tmp".to_string()),
            run: None,
            name: None,
        })
        .expect("a daemon that answered ping can make a workspace")
        .created
        .expect("workspace.create answers with the pane it started");

    // Not before the fixture is reading. It takes the terminal with `TCSAFLUSH`, the way a
    // full-screen harness does, and that discards input which arrived and has not been read -
    // so a message sent into the gap is thrown away and this would fail for the wrong reason.
    // Muster's own wait covers `pane new --run`; a `pane send` into a pane somebody else just
    // made is on its own, which is a real thing about `pane send` and not a quirk of the
    // fixture.
    wait_until_reading(&daemon, "w1:p1");

    // No Return. What is under test is the text, and a Return arriving behind it would put a
    // fourth thing on the wire for the fixture to have to account for.
    backend
        .submit(&BackendIntent::SendText {
            pane,
            text: "line one\nline two\nline three".to_string(),
            enter: false,
        })
        .expect("a real daemon takes text for a pane it holds");

    let got = until_some("the fixture to be handed the message", || {
        std::fs::read(received.join("handed")).ok().filter(|bytes| !bytes.is_empty())
    });
    let got = String::from_utf8_lossy(&got).into_owned();

    assert_eq!(
        got, "\u{1b}[200~line one\nline two\nline three\u{1b}[201~",
        "the message reached the pane unfenced, so a harness reading a bracketed paste takes \
         each newline in it as a submission and acts on `line three` alone.\n  Impact: an agent \
         told anything longer than one line is told its last line. `pane send` and the \
         keyboard's own paste must both go out on `pane.send_input`, which is the verb herdr \
         encodes against the pane's real modes - `pane.send_text` writes the bytes as given and \
         cannot fence anything.\n  Check `request` in muster-herdr/src/intent.rs and the \
         send_text cases in corpus/conformance/backend-intent.json."
    );

    let _ = std::fs::remove_dir_all(&received);
}

/// Waits for the fixture's own prompt, which is what says it has taken the terminal.
///
/// Through a client of its own because the harness's `call` is sized for a keystroke, and
/// `pane.wait_for_output` is the one call herdr answers slowly on purpose
/// (`observations/herdr-0.8.0.md` section 18).
fn wait_until_reading(daemon: &Daemon, pane: &str) {
    HerdrClient::new(daemon.socket_path().to_string_lossy().into_owned())
        .request_within(
            "pane.wait_for_output",
            &json!({
                "pane_id": pane,
                "match": { "type": "substring", "value": READING },
                "source": "visible",
                "timeout_ms": 10_000,
            }),
            Duration::from_secs(15),
        )
        .expect("the fixture prints a prompt once it is reading");
}

/// A directory this test owns, beside the daemon roots rather than inside one.
///
/// The fixture's path is written into the daemon's config, so it has to be known before the
/// daemon exists.
fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the harness root should be writable");
    path
}

/// A program that asks to be told about pastes, and writes down the bytes it is handed.
///
/// The bytes rather than the text, because the fence *is* the observation: a test that read
/// this back as a string would pass whether or not the daemon encoded anything.
///
/// It must not exit - herdr closes a pane whose process ends, then the workspace, then the
/// daemon - so it sleeps rather than returning once it has recorded a message.
fn fixture(received: &Path) -> PathBuf {
    let script = received.join("records-what-it-is-handed.py");
    std::fs::write(
        &script,
        format!(
            r#"#!/usr/bin/env python3
import os, select, sys, time, tty

RECEIVED = {received:?}

# Raw, and bracketed paste on: the shape of every harness `pane send` exists to talk to.
# Raw mode also keeps the line discipline from folding what arrives into lines of its own.
tty.setraw(0)
os.write(1, b"\x1b[?2004h")

# And say so, because taking the terminal is what discards anything sent before this line.
os.write(1, {reading:?}.encode() + b" ")

got = b""
quiet_since = None
while True:
    if select.select([0], [], [], 0.1)[0]:
        try:
            chunk = os.read(0, 65536)
        except OSError:
            chunk = b""
        if chunk:
            got += chunk
            quiet_since = time.monotonic()
            continue
    # Written once the sender has stopped, so a message that arrives in several reads is
    # one file rather than a growing one a reader could catch half way.
    if got and quiet_since is not None and time.monotonic() - quiet_since > 0.4:
        with open(os.path.join(RECEIVED, "handed"), "wb") as handed:
            handed.write(got)
        got = b""
        quiet_since = None
"#,
            received = received.to_string_lossy(),
            reading = READING,
        ),
    )
    .expect("the scratch directory should be writable");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the fixture should be executable");
    script
}
