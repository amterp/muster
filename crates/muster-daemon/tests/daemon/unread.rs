//! A pane's program that stops reading its input, as the daemon publishes it on the pane's
//! record (`Pane.unread_input`): real programs on a real terminal, each in one process group as
//! an agent is, so the evidence of reading the daemon keeps is about one program throughout.

use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};

/// How long input waits unread before a program that was reading is called stopped, in these
/// tests: a second, so that the programs below sleep a few seconds rather than half a minute.
const STUCK_MS: &str = "1000";

fn daemon() -> Daemon {
    daemon_with(&[("MUSTER_DAEMON_STUCK_MS", STUCK_MS)])
}

/// A pane running `script` under `sh -c`, once it has printed `ready`.
fn running(control: &mut Control, pane: &str, script: &str) {
    let create = proto::pane_request::Create {
        command: Some(format!("sh -c '{script}'")),
        ..create(pane, in_new_tab(&format!("t-{pane}")))
    };
    make(control, create);
    until_text(control, pane, "ready");
}

fn send(input: &mut Input, pane: &str, text: &str, enter: bool) {
    input.send(
        pane,
        Event::Send(input_event::Send { text: text.to_string(), enter, ..Default::default() }),
    );
}

fn unread(control: &mut Control, pane: &str) -> Option<proto::UnreadInput> {
    snapshot(control)
        .panes
        .into_iter()
        .find(|record| record.pane == pane)
        .unwrap_or_else(|| panic!("no pane {pane}"))
        .unread_input
}

/// Asserts the pane is never called stopped for `long`.
#[track_caller]
fn never_stopped(control: &mut Control, pane: &str, long: Duration) {
    let started = Instant::now();
    while started.elapsed() < long {
        let unread = unread(control, pane);
        assert_eq!(unread, None, "{pane} was called stopped at {:?}", started.elapsed());
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A program that has read its input and then leaves what is sent unread is published stopped,
/// with what waits and which program leads its group, and cleared once it reads again.
#[test]
fn a_program_that_stops_reading_is_published_until_it_reads_again() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    running(
        &mut control,
        "p1",
        "stty raw -echo; printf ready; head -c 1 >/dev/null; printf took; sleep 6; \
         printf reading; cat >/dev/null",
    );
    send(&mut input, "p1", "x", false);
    until_text(&mut control, "p1", "took");
    // Long enough for the daemon to have looked and found the keystroke taken, which is the
    // evidence that this program reads.
    std::thread::sleep(Duration::from_millis(1500));
    send(&mut input, "p1", "hello", false);

    let stopped = until_some("p1 to be published stopped", || unread(&mut control, "p1"));
    assert_eq!(stopped.bytes, 5, "{stopped:?}");
    assert_eq!(stopped.program, "sh", "{stopped:?}");
    assert!(stopped.pid > 0 && stopped.since_ms > 0, "{stopped:?}");

    until_text(&mut control, "p1", "reading");
    until_some("p1 to be published reading again", || {
        unread(&mut control, "p1").is_none().then_some(())
    });
}

/// A program that has not read anything yet - an agent still starting, with input sent as it
/// was launched - is never called stopped, however long that input waits.
#[test]
fn a_program_not_yet_reading_is_never_stopped() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    running(&mut control, "p1", "stty raw -echo; printf ready; sleep 4; cat >/dev/null");
    send(&mut input, "p1", "hello", true);
    never_stopped(&mut control, "p1", Duration::from_secs(6));
}

/// Read line by line, half a line counts as nothing waiting: a program that read a line and
/// waits for the next while half of it is typed is reading, not stopped.
#[test]
fn half_a_line_typed_is_not_input_left_unread() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    running(&mut control, "p1", "printf ready; read a; printf got; read b; sleep 30");
    send(&mut input, "p1", "one", true);
    until_text(&mut control, "p1", "got");
    send(&mut input, "p1", "two, half typed", false);
    never_stopped(&mut control, "p1", Duration::from_secs(4));
}

/// Pointer input for a program that has stopped reading never reaches it: neither what was
/// queued behind input it had not taken when it was called stopped, nor what arrived after.
/// What it reads once it reads again is what was typed, with no stale wheel turn in it.
#[test]
fn a_program_that_stops_reading_is_sent_none_of_the_pointer_input_meanwhile() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    // The alternate screen with alternate scroll turns a wheel turn into arrows, so the turn
    // needs no mouse report to be seen.
    running(
        &mut control,
        "p1",
        "stty raw -echo; printf \"\\033[?1049h\\033[?1007h\"; printf ready; \
         head -c 1 >/dev/null; printf took; sleep 8; cat >\"$HOME/heard\"",
    );
    send(&mut input, "p1", "x", false);
    until_text(&mut control, "p1", "took");
    std::thread::sleep(Duration::from_millis(1500));
    // More than any terminal holds, so the wheel turn behind it waits in the daemon's queue.
    let typed = "y".repeat(256 * 1024);
    input.send("p1", Event::Paste(input_event::Paste { text: typed.clone(), confirmed: true }));
    let wheel =
        || Event::Wheel(input_event::Wheel { dy: 1.0, x: 15.0, y: 25.0, ..Default::default() });
    input.send("p1", wheel());
    until_some("p1 to be published stopped", || unread(&mut control, "p1"));
    input.send("p1", wheel());
    send(&mut input, "p1", "end", false);

    let heard = daemon.root().join("home/heard");
    let read = until_some("p1 to read what was typed", || {
        std::fs::read(&heard).ok().filter(|read| read.ends_with(b"end"))
    });
    assert!(!read.contains(&0x1b), "a wheel turn reached the program");
    assert_eq!(read.len(), typed.len() + b"end".len(), "only what was typed was read");
}
