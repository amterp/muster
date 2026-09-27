//! The input connection: what a pane's program receives for keys, clicks, wheel turns, pastes,
//! `pane send` text and focus (MIP-3 section 6).
//!
//! Each pane here runs a program that puts its terminal in raw mode, sets whatever modes the
//! test is about, and copies everything it receives to a file. Expected bytes come from
//! libghostty's own encoders configured from a terminal fed the same modes, never written
//! out by hand, except where the decision is Ghostty's rather than an encoder's.

use std::path::{Path, PathBuf};

use crate::support::*;
use muster_core::input::{Key, KeyAction, KeyEvent, OptionAsAlt};
use muster_harness::Input;
use muster_vt::{KeyEncoder, KeyModes, MouseEncoder, MouseGeometry, Terminal, encode_paste};
use proto::event::Event as Payload;
use proto::input_event::{self, Input as Event};

/// libghostty's key codes, as a surface hands them over.
const KEY_A: u32 = 20;
const KEY_Z: u32 = 45;
const KEY_ESCAPE: u32 = 120;

const GRID: proto::Grid = proto::Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 };

/// A pane whose program sets `modes` and then saves what it receives.
fn receiving(control: &mut Control, daemon: &Daemon, name: &str, modes: &[u8]) -> PathBuf {
    let out = daemon.root().join(name);
    let command = format!(
        "stty raw -echo; printf '{}'; echo ready; cat > {}",
        printf_format(modes),
        out.display()
    );
    make(
        control,
        proto::pane_request::Create {
            command: Some(command),
            grid: Some(GRID),
            ..create(name, in_new_tab(&format!("t-{name}")))
        },
    );
    until_text(control, name, "ready");
    out
}

fn printf_format(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\x1b', "\\033").replace('%', "%%")
}

/// A terminal in the modes a pane's program set, for the encoders to be configured from.
fn after(modes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(80, 24).expect("a terminal");
    terminal.resize(80, 24, (10, 20)).expect("a size");
    terminal.write(modes);
    terminal
}

/// Waits until a pane's program has received exactly `expected`.
fn received(path: &Path, expected: &[u8]) {
    until(
        &format!("{} to hold {:?}", path.display(), String::from_utf8_lossy(expected)),
        || std::fs::read(path).is_ok_and(|bytes| bytes == expected),
        || {
            format!(
                "it holds {:?}",
                String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default())
            )
        },
    );
}

fn key(code: u32, text: &str) -> Event {
    Event::Key(input_event::Key {
        action: proto::KeyAction::Press.into(),
        key: code,
        text: text.to_string(),
        ..input_event::Key::default()
    })
}

fn mouse(action: proto::MouseAction, mods: u32) -> Event {
    Event::Mouse(input_event::Mouse {
        action: action.into(),
        button: proto::MouseButton::Left.into(),
        mods,
        x: 15.0,
        y: 25.0,
    })
}

fn wheel_up() -> Event {
    Event::Wheel(input_event::Wheel { dy: 1.0, x: 15.0, y: 25.0, ..input_event::Wheel::default() })
}

fn key_encoder(terminal: &Terminal) -> KeyEncoder {
    let mut encoder = KeyEncoder::new(KeyModes::default()).expect("an encoder");
    encoder.configure_from(terminal, OptionAsAlt::Never);
    encoder
}

fn mouse_encoder(terminal: &Terminal) -> MouseEncoder {
    let mut encoder = MouseEncoder::new().expect("an encoder");
    encoder.configure_from(terminal);
    encoder.set_geometry(MouseGeometry {
        screen_pixels: (800, 480),
        cell_pixels: (10, 20),
        padding: (0, 0, 0, 0),
    });
    encoder
}

fn mouse_event(
    action: muster_vt::MouseAction,
    button: muster_vt::MouseButton,
) -> muster_vt::MouseEvent {
    muster_vt::MouseEvent {
        action,
        button: Some(button),
        modifiers: muster_core::input::Modifiers::NONE,
        position: (15.0, 25.0),
    }
}

#[test]
fn keys_are_encoded_against_the_programs_own_modes() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let kitty = b"\x1b[>1u";
    let out = receiving(&mut control, &daemon, "p1", kitty);
    let mut input = Input::connect(daemon.socket_path());
    input.send("p1", key(KEY_A, "a"));
    input.send("p1", key(KEY_ESCAPE, ""));

    let encoder = key_encoder(&after(kitty));
    let mut expected =
        encoder.encode(&KeyEvent { text: "a".to_string(), ..KeyEvent::press(Key::KeyA) }).unwrap();
    expected.extend(encoder.encode(&KeyEvent::press(Key::Escape)).unwrap());
    assert_eq!(&expected[expected.len() - 5..], b"\x1b[27u", "kitty flags reached the encoder");
    received(&out, &expected);
}

#[test]
fn a_paste_is_fenced_when_asked_for_and_held_when_it_would_run_unasked() {
    let daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    let fenced = receiving(&mut control, &daemon, "fenced", b"\x1b[?2004h");
    let plain = receiving(&mut control, &daemon, "plain", b"");
    let mut input = Input::connect(daemon.socket_path());
    let paste = |text: &str, confirmed| {
        Event::Paste(input_event::Paste { text: text.to_string(), confirmed })
    };

    input.send("fenced", paste("one\ntwo", false));
    received(&fenced, &encode_paste("one\ntwo", true));

    input.send("plain", paste("x\ny", false));
    events_until(&mut control, "the paste held", |events| {
        events.iter().any(|event| {
            matches!(event.event.as_ref(), Some(Payload::PasteHeld(held))
                if held.pane == "plain" && held.text == "x\ny")
        })
    });
    input.send("plain", paste("single", false));
    input.send("plain", paste("x\ny", true));
    let mut expected = b"single".to_vec();
    expected.extend(encode_paste("x\ny", false));
    received(&plain, &expected);
}

#[test]
fn sent_text_goes_as_a_paste_and_is_submitted_with_a_return() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let fenced = receiving(&mut control, &daemon, "fenced", b"\x1b[?2004h");
    let plain = receiving(&mut control, &daemon, "plain", b"");
    let mut input = Input::connect(daemon.socket_path());
    let send = |text: &str| Event::Send(input_event::Send { text: text.to_string(), enter: true });
    input.send("fenced", send("line one\nline two"));
    input.send("plain", send("line one\nline two"));

    let enter = |terminal: &Terminal| {
        let encoder = key_encoder(terminal);
        let mut bytes = encoder.encode(&KeyEvent::press(Key::Enter)).unwrap();
        let release = KeyEvent { action: KeyAction::Release, ..KeyEvent::press(Key::Enter) };
        bytes.extend(encoder.encode(&release).unwrap());
        bytes
    };
    let mut expected = encode_paste("line one\nline two", true);
    expected.extend(enter(&after(b"\x1b[?2004h")));
    received(&fenced, &expected);
    let mut expected = b"line one\nline two".to_vec();
    expected.extend(enter(&after(b"")));
    received(&plain, &expected);
}

/// A send with no text and a Return is the Return alone. An empty paste before it is text
/// nobody sent, and a program that fences pastes may take the Return right after one as part
/// of the paste: a Claude Code prompt was left unsubmitted that way.
#[test]
fn an_empty_send_with_a_return_is_the_return_alone() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let fenced = receiving(&mut control, &daemon, "fenced", b"\x1b[?2004h");
    let mut input = Input::connect(daemon.socket_path());
    input.send("fenced", Event::Send(input_event::Send { text: String::new(), enter: true }));

    let encoder = key_encoder(&after(b"\x1b[?2004h"));
    let mut expected = encoder.encode(&KeyEvent::press(Key::Enter)).unwrap();
    let release = KeyEvent { action: KeyAction::Release, ..KeyEvent::press(Key::Enter) };
    expected.extend(encoder.encode(&release).unwrap());
    received(&fenced, &expected);
}

/// A long send reaches a program reading raw input whole, in however many writes the pty takes.
#[test]
fn a_long_send_reaches_a_raw_reader_whole() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = receiving(&mut control, &daemon, "raw", b"");
    let text = "0123456789".repeat(1000);
    let mut input = Input::connect(daemon.socket_path());
    input.send("raw", Event::Send(input_event::Send { text: text.clone(), enter: false }));
    received(&out, text.as_bytes());
}

/// A line sent to a program reading lines arrives as the terminal's line discipline delivers it:
/// the daemon writes to a real pty and adds no limit of its own. 1023 characters and a return
/// fit the smallest line buffer a kernel here keeps, macOS's 1024 bytes.
#[test]
fn a_line_sent_to_a_line_reader_arrives_whole() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = daemon.root().join("lines");
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(format!("echo ready; cat > {}", out.display())),
            grid: Some(GRID),
            ..create("lines", in_new_tab("t-lines"))
        },
    );
    until_text(&mut control, "lines", "ready");
    let line = "x".repeat(1023);
    let mut input = Input::connect(daemon.socket_path());
    input.send("lines", Event::Send(input_event::Send { text: line.clone(), enter: true }));
    received(&out, format!("{line}\n").as_bytes());
}

#[test]
fn a_click_is_reported_only_to_a_program_tracking_the_mouse_and_never_with_shift() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let tracking_modes = b"\x1b[?1000h\x1b[?1006h";
    let tracking = receiving(&mut control, &daemon, "tracking", tracking_modes);
    let plain = receiving(&mut control, &daemon, "plain", b"");
    let mut input = Input::connect(daemon.socket_path());
    let shift = u32::from(muster_core::input::Modifiers::SHIFT.0);
    for pane in ["tracking", "plain"] {
        input.send(pane, mouse(proto::MouseAction::Press, 0));
        input.send(pane, mouse(proto::MouseAction::Release, 0));
        input.send(pane, mouse(proto::MouseAction::Press, shift));
        input.send(pane, key(KEY_Z, "z"));
    }

    let mut encoder = mouse_encoder(&after(tracking_modes));
    let mut expected = encoder
        .encode(&mouse_event(muster_vt::MouseAction::Press, muster_vt::MouseButton::Left))
        .unwrap();
    expected.extend(
        encoder
            .encode(&mouse_event(muster_vt::MouseAction::Release, muster_vt::MouseButton::Left))
            .unwrap(),
    );
    expected.extend(b"z");
    received(&tracking, &expected);
    received(&plain, b"z");
}

/// A program that asked for shift-clicks with XTSHIFTESCAPE is sent them, shift and all.
#[test]
fn a_shift_click_reaches_a_program_that_asked_for_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let modes = b"\x1b[?1000h\x1b[?1006h\x1b[>1s";
    let asked = receiving(&mut control, &daemon, "asked", modes);
    let mut input = Input::connect(daemon.socket_path());
    let shift = u32::from(muster_core::input::Modifiers::SHIFT.0);
    input.send("asked", mouse(proto::MouseAction::Press, shift));

    let mut encoder = mouse_encoder(&after(modes));
    let shifted = muster_vt::MouseEvent {
        modifiers: muster_core::input::Modifiers::SHIFT,
        ..mouse_event(muster_vt::MouseAction::Press, muster_vt::MouseButton::Left)
    };
    received(&asked, &encoder.encode(&shifted).unwrap());
}

#[test]
fn a_wheel_turn_is_arrows_to_a_pager_a_report_to_a_mouse_program_and_nothing_otherwise() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let pager = receiving(&mut control, &daemon, "pager", b"\x1b[?1049h\x1b[?1007h");
    let tracking_modes = b"\x1b[?1000h\x1b[?1006h";
    let tracking = receiving(&mut control, &daemon, "tracking", tracking_modes);
    let plain = receiving(&mut control, &daemon, "plain", b"");
    let mut input = Input::connect(daemon.socket_path());
    for pane in ["pager", "tracking", "plain"] {
        input.send(pane, wheel_up());
        input.send(pane, key(KEY_Z, "z"));
    }

    // One discrete tick is three rows, Ghostty's default multiplier.
    received(&pager, b"\x1b[A\x1b[A\x1b[Az");
    let mut encoder = mouse_encoder(&after(tracking_modes));
    let up = encoder
        .encode(&mouse_event(muster_vt::MouseAction::Press, muster_vt::MouseButton::WheelUp))
        .unwrap();
    let mut expected = up.repeat(3);
    expected.extend(b"z");
    received(&tracking, &expected);
    received(&plain, b"z");
}

#[test]
fn focus_is_reported_only_to_a_program_that_asked() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let asked = receiving(&mut control, &daemon, "asked", b"\x1b[?1004h");
    let plain = receiving(&mut control, &daemon, "plain", b"");
    let mut input = Input::connect(daemon.socket_path());
    for pane in ["asked", "plain"] {
        input.send(pane, Event::Focus(input_event::Focus { focused: false }));
        input.send(pane, Event::Focus(input_event::Focus { focused: true }));
        input.send(pane, key(KEY_Z, "z"));
    }
    received(&asked, b"\x1b[O\x1b[Iz");
    received(&plain, b"z");
}

#[test]
fn a_bindings_bytes_are_written_and_a_reset_resets_the_pane_and_its_surface() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let out = receiving(&mut control, &daemon, "p1", b"hello");
    let mut input = Input::connect(daemon.socket_path());
    let perform = |action| {
        Event::Perform(input_event::Perform { action: Some(action), ..Default::default() })
    };
    input.send("p1", perform(input_event::perform::Action::Raw(b"\x1b[15~".to_vec())));
    received(&out, b"\x1b[15~");

    let mut stream = attached(&daemon, "p1", false);
    let mut surface = Surface::new(80, 24);
    surface.follow(&mut stream, "the replay", true, |surface| surface.replays > 0);
    assert!(surface.screen().contains("hello"));
    input.send("p1", perform(input_event::perform::Action::Reset(input_event::perform::Reset {})));
    surface.follow(&mut stream, "the reset", true, |surface| !surface.screen().contains("hello"));
    until_some("the daemon's copy to be reset too", || {
        Some(read_text(&mut control, "p1", 0, 0).text).filter(|text| !text.contains("hello"))
    });
    received(&out, b"\x1b[15~");
}

#[test]
fn keys_for_a_name_reach_the_pane_that_has_it_now() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    let first = receiving(&mut control, &daemon, "p1", b"");
    input.send("p1", key(KEY_A, "a"));
    received(&first, b"a");

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    let second = receiving(&mut control, &daemon, "p1", b"");
    input.send("p1", key(KEY_Z, "z"));
    received(&second, b"z");
}
