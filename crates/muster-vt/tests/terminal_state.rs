//! The state the daemon encodes input from and composes a replay from, read back after a
//! program set it. Each read is one libghostty call whose output type is ours to get right,
//! and a wrong type there reads garbage rather than failing - these are what would notice.

use muster_vt::{FormatOptions, Mode, Rgb, Screen, Terminal, TerminalOptions};

fn terminal_after(bytes: &[u8]) -> Terminal {
    let mut terminal = Terminal::new(20, 4).expect("libghostty-vt gives us a terminal");
    terminal.write(bytes);
    terminal
}

#[test]
fn modes_read_as_the_program_left_them() {
    let terminal = terminal_after(b"\x1b[?1h\x1b[?2004h\x1b[4h\x1b[?7l");
    assert!(terminal.mode(Mode::CURSOR_KEYS));
    assert!(terminal.mode(Mode::BRACKETED_PASTE));
    assert!(terminal.mode(Mode::INSERT), "an ANSI mode must be read as ANSI, not as ?4");
    assert!(!terminal.mode(Mode::WRAPAROUND));
    assert!(terminal.mode(Mode::GRAPHEME_CLUSTER), "on from creation");
}

#[test]
fn every_mode_in_the_header_is_one_the_library_answers_for() {
    assert!(Mode::all().count() > 30);
    assert!(Mode::all().any(|mode| mode == Mode::ALT_SCREEN_SAVE));
    assert_eq!(Mode::INSERT.name(), "INSERT");
}

#[test]
fn keyboard_mouse_and_screen_state() {
    let mut terminal = terminal_after(b"\x1b[>5u\x1b[?1002h");
    assert_eq!(terminal.kitty_keyboard_flags(), 5);
    assert!(terminal.mouse_tracking());

    // Kitty keyboard flags belong to a screen, so a replay has to state them once per screen
    // rather than once at the end.
    terminal.write(b"\x1b[?1049h");
    assert_eq!(terminal.active_screen(), Screen::Alternate);
    assert_eq!(terminal.kitty_keyboard_flags(), 0);
    terminal.write(b"\x1b[?1049l");
    assert_eq!(terminal.kitty_keyboard_flags(), 5);
}

#[test]
fn history_and_pending_wrap() {
    let mut terminal = terminal_after(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
    assert_eq!(terminal.scrollback_rows(), 2);
    assert_eq!(terminal.total_rows(), 6);
    assert_eq!((terminal.columns(), terminal.rows()), (20, 4));
    terminal.write(b"\r\n01234567890123456789");
    assert!(terminal.pending_wrap());
}

#[test]
fn a_program_color_is_an_override_and_the_default_is_kept() {
    let mut terminal = terminal_after(b"\x1b]4;1;rgb:12/34/56\x1b\\\x1b]11;rgb:01/02/03\x1b\\");
    assert_eq!(terminal.palette()[1], Rgb { r: 0x12, g: 0x34, b: 0x56 });
    assert_ne!(terminal.default_palette()[1], terminal.palette()[1]);
    assert_eq!(terminal.background(), Some(Rgb { r: 1, g: 2, b: 3 }));
    assert_ne!(terminal.background(), terminal.default_background());

    let mut theme = [Rgb { r: 9, g: 9, b: 9 }; 256];
    theme[1] = Rgb { r: 1, g: 1, b: 1 };
    terminal.set_default_palette(&theme);
    assert_eq!(terminal.palette()[2], Rgb { r: 9, g: 9, b: 9 }, "an untouched entry follows");
    assert_eq!(terminal.palette()[1], Rgb { r: 0x12, g: 0x34, b: 0x56 }, "an override stays");
}

#[test]
fn title_and_directory() {
    let terminal = terminal_after(b"\x1b]2;building\x1b\\\x1b]7;file://host/tmp\x1b\\");
    assert_eq!(terminal.title(), "building");
    assert_eq!(terminal.pwd(), "file://host/tmp");
}

#[test]
fn the_formatter_reads_history_and_screen_in_one_call() {
    let terminal = terminal_after(b"one\r\ntwo\r\nthree\r\nfour\r\n\x1b[1mfive\x1b[0m");
    let plain = String::from_utf8(terminal.format(FormatOptions::plain())).expect("utf-8");
    assert_eq!(plain, "one\ntwo\nthree\nfour\nfive");
    let vt = String::from_utf8(terminal.format(FormatOptions::vt())).expect("utf-8");
    assert!(vt.contains("\x1b[0m\x1b[1mfive"), "styles come through as SGR: {vt:?}");
}

#[test]
fn a_terminal_moves_to_another_thread() {
    let terminal = terminal_after(b"hello");
    let text = std::thread::spawn(move || terminal.format(FormatOptions::plain()))
        .join()
        .expect("the thread finishes");
    assert_eq!(text, b"hello");
}

#[test]
fn scrollback_keeps_what_the_byte_limit_allows() {
    // libghostty's default keeps about a thousand rows at this width, and prunes in pages of
    // about 400 KB, so the limits here are far apart on purpose.
    let lines: Vec<u8> = (0..20_000).flat_map(|n| format!("line {n}\r\n").into_bytes()).collect();
    let rows_kept = |bytes: usize| {
        let mut terminal = Terminal::with_options(TerminalOptions {
            scrollback_bytes: Some(bytes),
            ..TerminalOptions::new(80, 24)
        })
        .expect("libghostty-vt gives us a terminal");
        terminal.write(&lines);
        terminal.scrollback_rows()
    };
    assert!(rows_kept(64 * 1024 * 1024) > 19_000);
    assert_eq!(rows_kept(0), 0);
}
