//! What `muster pane read --rows N` hands back, against a real daemon.
//!
//! The one thing no corpus can pin. `PaneText::tail` is a pure count and has cases of its own;
//! what needs a daemon is the claim underneath it - that `pane.read` counts *grid* rows, so the
//! blank remainder of an idle viewport is rows to herdr and a small number asked for over the
//! wire buys those and nothing else.
//!
//! That is the bug this file exists for. `--rows 12` on a shell sitting at a prompt answered
//! with `""` and exit 0, which is byte-identical to a pane that has printed nothing - and the
//! pane you most often read is exactly the one you are asking whether it is quiet. Muster now
//! asks for as far back as herdr will go and counts here.
//!
//! The pane is held by a control stream throughout, for the reason `find.rs` gives: attaching
//! one sets the pane's geometry, and the whole of this test is about a viewport much taller
//! than what has been printed into it.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use herdr_harness::{Daemon, until_within};
use muster_core::intent::BackendChannel;
use muster_core::mirror::backend::PaneId;
use serde_json::json;

/// How many rows the fixture prints. Far fewer than the viewport below, which is the whole
/// shape of the bug: a pane with a little on it and a lot of blank underneath.
const PRINTED: u32 = 20;

/// The viewport the control stream sets. Tall, so that most of the pane is the blank remainder
/// - the same shape as the 79-row pane holding 24 lines that this was reported on.
const VIEWPORT: &str = "60";

/// A tall pane holding `PRINTED` numbered rows and a great deal of nothing.
struct Quiet {
    daemon: Daemon,
    pane: PaneId,
    stream: Child,
}

impl Quiet {
    fn new() -> Quiet {
        let daemon = Daemon::start();
        daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
        let pane = PaneId::new("w1:p1");
        let stream = Command::new(herdr_harness::binary())
            .args([
                "terminal",
                "session",
                "control",
                pane.as_str(),
                "--cols",
                "80",
                "--rows",
                VIEWPORT,
            ])
            .env("HERDR_SOCKET_PATH", daemon.socket_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the pinned herdr can open a control session");
        let quiet = Quiet { daemon, pane, stream };

        // One awk rather than a shell loop, and the command's own echo carries no `line-`, so
        // the rows below are only the rows.
        let script =
            format!("awk 'BEGIN{{for(i=1;i<={PRINTED};i++) printf \"line-%03d\\n\", i}}'\n");
        quiet
            .daemon
            .call("pane.send_text", &json!({ "pane_id": quiet.pane.as_str(), "text": script }));
        let last = format!("line-{PRINTED:03}");
        until_within(
            "the fixture to finish printing",
            Duration::from_secs(20),
            || quiet.whole().contains(&last),
            (),
        );
        quiet
    }

    /// Everything the pane will give, which is what every read asks for now.
    fn whole(&self) -> String {
        self.daemon.backend().read(&self.pane).expect("herdr answered").text
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        let _ = self.stream.kill();
        let _ = self.stream.wait();
    }
}

/// The premise the fix rests on, asked of the daemon directly.
///
/// Muster no longer sends a small `lines`, so nothing else here would notice if herdr started
/// counting rows of text instead of rows of the grid - and the whole shape of the fix would
/// then be unnecessary rather than wrong. This is what says which world we are in: on 0.8.0 a
/// count below the viewport height answers with nothing at all, on a pane holding twenty rows.
#[test]
fn herdr_counts_grid_rows_which_is_why_a_small_count_bought_nothing() {
    let quiet = Quiet::new();

    let answered = quiet.daemon.call(
        "pane.read",
        &json!({
            "pane_id": quiet.pane.as_str(),
            "source": "recent",
            "lines": 5,
            "strip_ansi": true,
        }),
    );
    let text = answered.get("text").and_then(|text| text.as_str()).unwrap_or_default();

    assert!(
        text.trim().is_empty(),
        "asking herdr for five rows of a sixty-row pane holding twenty answered {text:?}. If \
         that is now the last five printed rows, `PaneText::tail` and this whole file are \
         solving a problem the daemon has fixed - check the pin."
    );
    assert!(
        quiet.whole().contains(&format!("line-{PRINTED:03}")),
        "the same pane, asked without a small count, does hold what was printed"
    );
}

/// The reported bug, as a test.
///
/// Before this, a count smaller than the viewport answered with nothing at all: herdr counted
/// the blank rows at the bottom of the pane and they trimmed away. The number here is far
/// below `VIEWPORT`, which is the whole point - anything above it would have passed all along.
#[test]
fn a_small_count_on_a_quiet_pane_answers_with_its_last_rows() {
    let quiet = Quiet::new();

    let read = quiet.daemon.backend().read(&quiet.pane).expect("herdr answered").tail(5);
    let rows: Vec<&str> = read.text.lines().collect();

    assert_eq!(rows.len(), 5, "five rows were asked for and {} came back: {read:?}", rows.len());
    assert!(
        rows.iter().any(|row| row.contains(&format!("line-{PRINTED:03}"))),
        "the newest printed row is what the last five rows of this pane are: {rows:?}"
    );
    assert!(read.truncated, "fifteen printed rows were left above, which the flag has to say");
}

/// The other half, and the reason the fix is a count rather than a bigger ceiling: what a
/// caller gets back is the *newest* rows, which is what "the last forty" means to whoever
/// typed it.
#[test]
fn a_count_takes_the_newest_rows_a_pane_printed() {
    let quiet = Quiet::new();

    let read = quiet.daemon.backend().read(&quiet.pane).expect("herdr answered").tail(3);

    assert!(
        !read.text.contains("line-001"),
        "the oldest row is not among the last three: {read:?}"
    );
    assert!(read.text.contains(&format!("line-{PRINTED:03}")), "the newest row is: {read:?}");
}

/// No `--rows` still means the whole pane, and it is still the answer a small count now
/// agrees with rather than contradicts - which is what the two doc pages were disagreeing
/// about.
#[test]
fn no_count_still_answers_with_everything_the_pane_holds() {
    let quiet = Quiet::new();

    let read = quiet.daemon.backend().read(&quiet.pane).expect("herdr answered").tail(0);

    assert!(read.text.contains("line-001"), "the oldest printed row is there: {read:?}");
    assert!(read.text.contains(&format!("line-{PRINTED:03}")), "and so is the newest: {read:?}");
}
