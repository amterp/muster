//! The detection text a daemon hands the detector, read from the terminal it runs.
//!
//! herdr built its detection text row by row from libghostty (`src/pane/terminal.rs`,
//! `ghostty_detection_text`): the active screen's rows, whatever the viewport shows, each with
//! trailing whitespace trimmed and blank cells as spaces, trailing blank rows dropped, joined
//! and ended with a newline. `Pane::screen_text` is documented as `Terminal::text(0, rows - 1)`,
//! and these pin that the two agree, since the manifests' regions are cut on that text.

use muster_detect::screen_text;
use muster_vt::Terminal;

fn detection_text(columns: u16, rows: u16, bytes: &[u8]) -> String {
    let mut terminal = Terminal::new(columns, rows).expect("libghostty-vt gives us a terminal");
    terminal.write(bytes);
    screen_text(&terminal.text(0, terminal.rows() - 1))
}

#[test]
fn rows_lose_trailing_space_and_keep_leading_space() {
    assert_eq!(detection_text(20, 3, b"  indented   \r\nnext"), "  indented\nnext\n");
}

#[test]
fn blank_rows_inside_are_kept_and_blank_rows_below_are_dropped() {
    assert_eq!(detection_text(10, 6, b"a\r\n\r\nb\r\n"), "a\n\nb\n");
}

#[test]
fn a_gap_the_cursor_jumped_over_is_spaces() {
    assert_eq!(detection_text(20, 2, b"a\x1b[5Cb"), "a     b\n");
}

#[test]
fn history_above_the_screen_is_not_read() {
    assert_eq!(detection_text(10, 3, b"1\r\n2\r\n3\r\n4\r\n5"), "3\n4\n5\n");
}

#[test]
fn the_alternate_screen_is_read_while_it_is_up() {
    assert_eq!(detection_text(20, 3, b"primary\x1b[?1049h\x1b[Halternate"), "alternate\n");
    assert_eq!(detection_text(20, 3, b"primary\x1b[?1049h\x1b[Halternate\x1b[?1049l"), "primary\n");
}

#[test]
fn a_wide_character_is_one_character() {
    assert_eq!(detection_text(20, 2, "你好 ❯ x".as_bytes()), "你好 ❯ x\n");
}

#[test]
fn an_empty_screen_is_empty_text() {
    assert_eq!(detection_text(10, 3, b""), "");
}
