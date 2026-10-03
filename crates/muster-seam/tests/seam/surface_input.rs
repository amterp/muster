//! What the window's surfaces hand the core besides keystrokes, and what the core hands back.
//!
//! Each pane's surface scrolls, selects and searches by itself, so the core no longer does any
//! of that. What it still does is carry what the program should get - a wheel turn, a click - to
//! the pane's daemon, which decides from the program's modes whether it gets anything, and carry
//! a program's clipboard write back to the window, under the config's say-so.

use std::sync::Mutex;

use muster::proto::{
    AttachPane, ClipboardWrite, Event, KeyDown, KeyEvent, Mouse, OpenWindow, ReadPane, ReadWindow,
    Request, Response, Startup, Wheel, event, request, response,
};
use muster_daemon_proto::{Grid, pane_request};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

/// A press that only moves a composition along is not the pane's, and the shell is told so,
/// since it hands the pane's surface exactly the keys the program gets.
#[test]
fn a_press_says_whether_the_pane_got_it() {
    let _turn = muster::testing::fresh_session();
    let daemon = daemon_running(None);

    let _pane = open_on(&daemon, "");
    let typed = press("KeyA", "a", false);
    assert!(typed, "a letter nobody bound goes to the pane");
    let composing = press("KeyA", "a", true);
    assert!(!composing, "a press inside a composition sends the pane nothing");
}

/// A wheel turn over a program on the alternate screen that asked for no mouse reaches it as
/// arrow keys, scaled by `scroll_multiplier` as the surface scales its own scrolling, which the
/// daemon learns from the config.
#[test]
fn a_wheel_over_a_full_screen_program_reaches_it_as_arrows() {
    let _turn = muster::testing::fresh_session();
    let daemon = daemon_running(Some(&format!("printf '\\033[?1049h'; {HEX}")));
    let pane = open_on(&daemon, "scroll_multiplier = 2");
    until("the program to take the terminal", || read(&pane).contains("READY"), || read(&pane));

    assert_ok(&answer(request::Payload::Wheel(Wheel {
        daemon_id: "local".to_string(),
        pane_id: pane.clone(),
        dy: -1.0,
        ..Wheel::default()
    })));
    let arrows =
        || read(&pane).matches("1b 5b 42").count() + read(&pane).matches("1b 4f 42").count();
    until(
        "one notch at twice the scale to reach the program as six arrows down",
        || arrows() == 6,
        || format!("the pane shows {:?}", read(&pane)),
    );
}

/// A click over a program that asked for the mouse reaches it as a mouse report.
#[test]
fn a_click_reaches_a_program_that_asked_for_the_mouse() {
    let _turn = muster::testing::fresh_session();
    let daemon = daemon_running(Some(&format!("printf '\\033[?1000h'; {HEX}")));
    let pane = open_on(&daemon, "");
    until("the program to take the terminal", || read(&pane).contains("READY"), || read(&pane));

    assert_ok(&answer(request::Payload::Mouse(Mouse {
        daemon_id: "local".to_string(),
        pane_id: pane.clone(),
        action: "press".to_string(),
        button: "left".to_string(),
        x: 5.0,
        y: 5.0,
        ..Mouse::default()
    })));
    until(
        "the press to reach the program as a mouse report",
        || read(&pane).contains("1b 5b 4d"),
        || format!("the pane shows {:?}", read(&pane)),
    );
}

static WRITES: Mutex<Vec<ClipboardWrite>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ClipboardWrite(write)) = event.payload {
        WRITES.lock().expect("a panicking test poisoned the log").push(write);
    }
}

const COPIES: &str = "printf '\\033]52;c;%s\\007' \"$(printf copied | base64)\"";

/// A program that sets the clipboard reaches the window with what it set, since the window is
/// what owns a pasteboard.
#[test]
fn a_programs_clipboard_write_reaches_the_window() {
    let _turn = muster::testing::fresh_session();
    WRITES.lock().unwrap().clear();
    muster::ffi::muster_set_event_callback(Some(note));
    let daemon = daemon_running(Some(&format!("{COPIES}; exec cat")));
    let pane = open_on(&daemon, "");

    until(
        "the write to reach the window",
        || {
            WRITES
                .lock()
                .unwrap()
                .iter()
                .any(|write| write.pane_id == pane && write.text == "copied")
        },
        || format!("the window heard {:?}", WRITES.lock().unwrap()),
    );
    muster::ffi::muster_set_event_callback(None);
}

/// `clipboard_write = "deny"` keeps a program off the clipboard.
#[test]
fn a_denied_clipboard_write_goes_nowhere() {
    let _turn = muster::testing::fresh_session();
    WRITES.lock().unwrap().clear();
    muster::ffi::muster_set_event_callback(Some(note));
    let daemon = daemon_running(Some(&format!("{COPIES}; echo WROTE; exec cat")));
    let pane = open_on(&daemon, "clipboard_write = \"deny\"");

    until("the program to have written", || read(&pane).contains("WROTE"), || read(&pane));
    // The effect travels ahead of the output after it, on the one connection, so by the time
    // the text is readable the write has been decided on.
    assert!(WRITES.lock().unwrap().is_empty(), "{:?}", WRITES.lock().unwrap());
    muster::ffi::muster_set_event_callback(None);
}

/// Takes the terminal raw, says READY, and prints each byte it reads in hex, unbuffered: `cat
/// -v` holds what it read until a newline, and the escape sequences under test have none.
const HEX: &str = "stty raw -echo; exec perl -e '$|=1; print \"READY \"; while (sysread STDIN, $c, 1) { printf \"%02x \", ord $c }'";

/// A daemon holding one pane, running `program` in its shell when there is one.
///
/// Made at a grid with pixels, as a bridge reports one: a click is placed on a cell by its
/// pixels, and a pane no surface has drawn has none.
fn daemon_running(program: Option<&str>) -> Daemon {
    let daemon = Daemon::start_built();
    make(
        &mut daemon.connect(),
        pane_request::Create {
            command: program.map(str::to_string),
            grid: Some(Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 }),
            ..create("p1", in_new_tab("t1"))
        },
    );
    daemon
}

/// Opens a window on `daemon` with `config` in its file, and attaches its pane as a surface
/// does.
fn open_on(daemon: &Daemon, config: &str) -> String {
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config_with(config).to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    let mut found = None;
    until(
        "the window to hold the daemon's pane",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(ReadWindow::default())).payload
            else {
                return false;
            };
            found = window.panes.first().map(|pane| pane.pane_id.clone());
            found.is_some()
        },
        || "the window never listed a pane".to_string(),
    );
    let pane = found.unwrap();
    let attached = answer(request::Payload::AttachPane(AttachPane { pane_id: pane.clone() }));
    assert!(
        matches!(attached.payload, Some(response::Payload::Attached(_))),
        "attaching the pane answered {attached:?}"
    );
    pane
}

/// Presses a key on the pane with the keyboard, inside a composition that goes on after it or
/// not, and returns what the core said came of it.
fn press(key: &str, text: &str, composing: bool) -> bool {
    let down = KeyDown {
        key: Some(KeyEvent {
            action: "press".to_string(),
            key: key.to_string(),
            text: text.to_string(),
            ..KeyEvent::default()
        }),
        was_composing: composing,
        still_composing: composing,
        ..KeyDown::default()
    };
    match answer(request::Payload::KeyDown(down)).payload {
        Some(response::Payload::KeyHandled(handled)) => handled.to_pane,
        other => panic!("a press answered {other:?}"),
    }
}

fn read(pane: &str) -> String {
    match answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
    .payload
    {
        Some(response::Payload::PaneText(text)) => text.text,
        other => panic!("reading the pane answered {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice())
        .expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
