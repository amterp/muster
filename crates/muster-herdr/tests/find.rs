//! Finding text in a real pane's history, and the arithmetic that turns a hit into a scroll.
//!
//! `find.json` pins the matching and `backend-intent.json` pins the request, and neither
//! says a daemon answers with rows that line up with what it scrolls in. That alignment is
//! the whole of the feature's positioning - a hit's row *is* the offset to scroll to - and
//! it is a fact about herdr rather than about Muster, so only a daemon can be asked
//! (`observations/herdr-0.8.0.md` section 17).
//!
//! The pane is held by a control stream throughout, because attaching one sets its geometry:
//! a stream opened after the rows were printed would reflow them and every offset measured
//! before it would be about a pane that no longer exists.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use herdr_harness::{Daemon, until, until_within};
use muster_core::find::{Needle, Reach};
use muster_core::input::ScrollDirection;
use muster_core::intent::BackendChannel;
use muster_core::mirror::backend::PaneId;
use muster_herdr::ControlStreamMessage;
use serde_json::json;

/// How many numbered rows the fixture prints. Comfortably inside herdr's thousand-row read,
/// so what these measure is the alignment rather than the cap.
const ROWS: u32 = 300;

/// A pane holding `ROWS` numbered rows, with a live control stream to scroll it by.
struct Ruler {
    daemon: Daemon,
    pane: PaneId,
    stream: Child,
}

impl Ruler {
    fn new() -> Ruler {
        let daemon = Daemon::start();
        daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "one", "focus": true }));
        let pane = PaneId::new("w1:p1");
        let stream = attach(&daemon, &pane);
        let ruler = Ruler { daemon, pane, stream };

        // One awk rather than a shell loop: three hundred forks through a PTY is slow, and
        // the command's own echo carries no `ruler-0`, so the rows below are only the rows.
        let script = format!("awk 'BEGIN{{for(i=1;i<={ROWS};i++) printf \"ruler-%05d\\n\", i}}'\n");
        ruler
            .daemon
            .call("pane.send_text", &json!({ "pane_id": ruler.pane.as_str(), "text": script }));
        let last = format!("ruler-{ROWS:05}");
        // Longer than the suite's usual patience, and this is the one wait that earns it:
        // everything else here waits on a daemon that is already answering, where this waits
        // on three hundred rows travelling through a PTY and being read back a screen at a
        // time. Thirty seconds is what this test was written with; it is kept rather than
        // trimmed because a shorter one here can only add a flake.
        until_within(
            "the ruler to finish printing",
            Duration::from_secs(30),
            || ruler.visible().contains(&last),
            (),
        );
        ruler
    }

    /// Types into the pane and waits for `settled` to appear on its screen.
    ///
    /// Waited for rather than slept through, because everything after it reads the pane back
    /// and a read taken mid-repaint is a screen nobody ever saw.
    fn run(&self, command: &str, settled: &str) {
        self.daemon
            .call("pane.send_text", &json!({ "pane_id": self.pane.as_str(), "text": command }));
        until_within(
            &format!("{settled:?} to reach the pane"),
            Duration::from_secs(30),
            || self.visible().contains(settled),
            (),
        );
    }

    fn visible(&self) -> String {
        let answer = self.daemon.call(
            "pane.read",
            &json!({ "pane_id": self.pane.as_str(), "source": "visible", "strip_ansi": true }),
        );
        answer["read"]["text"].as_str().unwrap_or_default().to_string()
    }

    fn offset(&self) -> u64 {
        let answer = self.daemon.call("pane.get", &json!({ "pane_id": self.pane.as_str() }));
        answer["pane"]["scroll"]["offset_from_bottom"].as_u64().unwrap_or_default()
    }

    /// Moves the viewport the only way herdr offers: relative steps on the control stream.
    ///
    /// Built with Muster's own message rather than hand-written JSON, so a test that passes
    /// is a test the shipped envelope passed.
    fn scroll_up(&mut self, rows: u16) {
        let message = ControlStreamMessage::Scroll { direction: ScrollDirection::Up, lines: rows };
        let stdin = self.stream.stdin.as_mut().expect("the control stream takes input");
        stdin.write_all(&message.wire_format()).expect("the control stream is still open");
        stdin.flush().expect("the control stream is still open");
    }
}

impl Drop for Ruler {
    fn drop(&mut self) {
        let _ = self.stream.kill();
        let _ = self.stream.wait();
    }
}

/// A control session on the pane, which is what a visible pane always has in the real app.
///
/// Spawned here rather than through `muster-bridge`'s test support, because what is under
/// test is one adapter and pulling in the bridge would make this a test of three things.
fn attach(daemon: &Daemon, pane: &PaneId) -> Child {
    Command::new(herdr_harness::binary())
        .args(["terminal", "session", "control", pane.as_str(), "--cols", "80", "--rows", "24"])
        .env("HERDR_SOCKET_PATH", daemon.socket_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the pinned herdr can open a control session")
}

#[test]
fn a_needle_is_found_where_the_pane_actually_holds_it() {
    let ruler = Ruler::new();

    let found = ruler
        .daemon
        .backend()
        .find(&ruler.pane, &Needle::new("ruler-00042"))
        .expect("herdr answered");

    assert_eq!(found.hits.len(), 1, "exactly one row carries that number");
    let hit = &found.hits[0];
    assert_eq!(hit.matched, 0..11, "the number is the whole row");
    // Row 42 of 300 has 258 ruler rows below it, plus whatever the shell drew afterwards -
    // which belongs to /bin/sh rather than to anything this test controls, so the assertion
    // is a floor rather than a number.
    assert!(
        hit.rows_from_bottom >= ROWS - 42,
        "row 42 of {ROWS} sits at least {} rows up, not {}",
        ROWS - 42,
        hit.rows_from_bottom,
    );
    assert!(found.rows_searched >= ROWS, "the read reached all {ROWS} printed rows");
    assert_eq!(
        found.reach(),
        Reach::Whole,
        "{ROWS} rows is well inside what herdr answers with, and the pane has scrollback"
    );
}

#[test]
fn matches_come_back_bottom_most_first() {
    let ruler = Ruler::new();

    let found =
        ruler.daemon.backend().find(&ruler.pane, &Needle::new("ruler-0")).expect("herdr answered");

    assert_eq!(found.hits.len(), ROWS as usize, "one match per printed row and none in the echo");
    let offsets: Vec<u32> = found.hits.iter().map(|hit| hit.rows_from_bottom).collect();
    let mut climbing = offsets.clone();
    climbing.sort_unstable();
    assert_eq!(offsets, climbing, "the bottom-most match comes first and the index climbs");
}

#[test]
fn a_hit_scrolled_to_is_a_hit_on_screen() {
    // The claim the whole feature rests on. If herdr's scroll rows and its read's lines ever
    // stop being the same unit, this is the test that says so - everything above it would go
    // on passing while the window landed somewhere near the answer.
    let mut ruler = Ruler::new();
    let wanted = "ruler-00042";
    let found =
        ruler.daemon.backend().find(&ruler.pane, &Needle::new(wanted)).expect("herdr answered");
    let rows_up = found.hits.first().expect("the ruler printed that row").rows_from_bottom;

    ruler.scroll_up(u16::try_from(rows_up).expect("a 300-row pane is well inside one scroll"));
    until(
        "the daemon to report the offset it was asked for",
        || ruler.offset() == u64::from(rows_up),
        (),
    );

    assert!(
        ruler.visible().contains(wanted),
        "scrolling {rows_up} rows up should show {wanted}, and the screen was:\n{}",
        ruler.visible(),
    );
}

#[test]
fn a_pane_that_is_not_there_refuses_rather_than_answering_empty() {
    // "Nothing found" and "nobody looked" are different things to put under a search box, and
    // this is also how a window learns it is showing a pane the daemon has dropped.
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "focus": true }));

    let answer = daemon.backend().find(&PaneId::new("w1:p99"), &Needle::new("anything"));

    assert!(answer.is_err(), "a missing pane is a refusal, not an empty result");
}

#[test]
fn a_hit_above_a_blank_bottom_is_still_scrolled_to() {
    // The regression this file did not have a case for. herdr trims the blank remainder of
    // the viewport off the bottom of a read, so a pane whose program left the lower half of
    // the screen empty answers with fewer rows than it holds - and every offset measured
    // from that answer is short by the difference. Before the correction the landing here
    // stopped twenty-odd rows below the match, on a screen that does not carry it.
    //
    // `ESC[2J ESC[H` rather than `clear`, and the difference is the whole fixture: `clear`
    // sends `ESC[3J` as well, which erases the scrollback and leaves a pane with nothing to
    // scroll through.
    let mut ruler = Ruler::new();
    ruler.run("printf '\\033[2J\\033[H'; printf 'TOP\\n'\n", "TOP");

    let wanted = "ruler-00042";
    let found =
        ruler.daemon.backend().find(&ruler.pane, &Needle::new(wanted)).expect("herdr answered");
    let hit = found.hits.first().expect("the ruler printed that row").rows_from_bottom;
    assert!(
        found.rows_searched < found.rows_read,
        "the fixture has to leave blank rows below the last printed one, and this read of \
         {} rows covered {} grid rows",
        found.rows_searched,
        found.rows_read,
    );

    ruler.scroll_up(u16::try_from(hit).expect("a 300-row pane is well inside one scroll"));
    until(
        "the daemon to report the offset it was asked for",
        || ruler.offset() == u64::from(hit),
        (),
    );

    assert!(
        ruler.visible().contains(wanted),
        "scrolling {hit} rows up should show {wanted}, and the screen was:\n{}",
        ruler.visible(),
    );
}

#[test]
fn a_pane_on_the_alternate_screen_says_it_holds_only_the_screen() {
    // What every agent pane looks like, and the case that made find look broken. A
    // full-screen program leaves the daemon no history at all: `max_offset_from_bottom`
    // drops to zero and a read comes back as the visible rows, with nothing to say the
    // scrollback behind them is out of reach. Reported as a complete search of two hundred
    // rows, "no results" reads as "that text is not in this pane".
    let ruler = Ruler::new();
    let before = ruler.daemon.backend().find(&ruler.pane, &Needle::new("ruler-00042"));
    assert_eq!(before.expect("herdr answered").hits.len(), 1, "the row is there beforehand");

    ruler.run("printf '\\033[?1049h'; printf 'FULLSCREEN\\n'\n", "FULLSCREEN");

    let found = ruler
        .daemon
        .backend()
        .find(&ruler.pane, &Needle::new("ruler-00042"))
        .expect("herdr answered");
    assert!(found.hits.is_empty(), "the row is behind a screen the daemon no longer holds");
    assert_eq!(
        found.reach(),
        Reach::ScreenOnly,
        "a pane with nothing behind its screen has to say so, or its empty answer reads as a \
         complete one"
    );
}

#[test]
fn a_pane_deeper_than_herdr_will_read_says_how_deep_it_is() {
    // herdr clamps a read to a thousand rows and offers no way to page past them, so a long
    // pane is searched in part. The bar draws "last 1000 of 3000" from this, which is the
    // difference between a search that found nothing and one that did not look everywhere.
    let ruler = Ruler::new();
    ruler.run("awk 'BEGIN{for(i=1;i<=1500;i++) printf \"deep-%05d\\n\", i}'\n", "deep-01500");

    let found = ruler
        .daemon
        .backend()
        .find(&ruler.pane, &Needle::new("deep-00001"))
        .expect("herdr answered");

    assert!(found.hits.is_empty(), "the first deep row is past herdr's thousand-row ceiling");
    match found.reach() {
        Reach::Capped { rows_held } => assert!(
            rows_held > found.rows_read,
            "a capped search holds more rows ({rows_held}) than it read ({})",
            found.rows_read,
        ),
        other => panic!("a pane of 1800 rows read a thousand at a time is capped, not {other:?}"),
    }
}
