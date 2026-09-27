//! Grapheme clustering is the terminal's default, not a setting a program can wipe.
//!
//! Ghostty makes DEC mode 2027 a reset default from its config, and herdr patches it in as
//! one (`vendor/libghostty-vt.patches.md`). A terminal that only *set* the mode at creation
//! agrees with both until a program sends RIS - `reset`, `tput reset`, a crashed TUI's
//! cleanup - and from then on lays out a ZWJ emoji across several cells while the surface
//! beside it draws one. Agent detection reads this terminal's grid, so it would be reading a
//! screen nobody sees.

use muster_vt::{Terminal, Width};

const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";

fn first_cell_after(terminal: &mut Terminal, bytes: &[u8]) -> (String, Width) {
    terminal.write(bytes);
    let grid = terminal.viewport(10, 1);
    let cell = &grid.rows[0].cells[0];
    (cell.text.clone(), cell.width)
}

#[test]
fn a_zwj_sequence_is_one_cell_in_a_new_terminal() {
    let mut terminal = Terminal::new(10, 1).expect("libghostty-vt gives us a terminal");
    let (text, width) = first_cell_after(&mut terminal, FAMILY.as_bytes());
    assert_eq!(text, FAMILY);
    assert_eq!(width, Width::Wide);
}

#[test]
fn a_zwj_sequence_is_still_one_cell_after_a_full_reset() {
    let mut terminal = Terminal::new(10, 1).expect("libghostty-vt gives us a terminal");
    let mut bytes = b"\x1bc".to_vec();
    bytes.extend_from_slice(FAMILY.as_bytes());
    let (text, width) = first_cell_after(&mut terminal, &bytes);
    assert_eq!(text, FAMILY, "RIS turned grapheme clustering off");
    assert_eq!(width, Width::Wide);
}
