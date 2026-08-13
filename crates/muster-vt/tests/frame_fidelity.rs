//! Two renderers, one screen, and they have to agree.
//!
//! The grid harness claims that replaying a pane's frames tells us what the user sees. That
//! is a claim about herdr's frames *and* about the reading code here, and a snapshot cannot
//! check either - it only pins whatever we produced on the day.
//!
//! So the corpus records both halves of the same moment: the frames herdr sent, and herdr's
//! own text of the screen those frames describe (`pane.read`). Any disagreement is a real
//! defect in one of the two, which is exactly the check `docs/testing.md` wants standing
//! between the fast tier and reality.

mod support;

use muster_herdr::{FrameDecoder, PaneStreamEvent};
use muster_vt::{Grid, Width};
use support::{corpus_file, corpus_text};

fn replay_fidelity_frames() -> Grid {
    let stream = corpus_file("herdr-0.8.0/frame-fidelity/frames.ndjson");
    let mut decoder = FrameDecoder::new();
    let terminal = muster_vt::Terminal::new(80, 24).expect("libghostty-vt gives us a terminal");
    for event in decoder.consume(&stream) {
        if let PaneStreamEvent::Frame(frame) = event {
            terminal.write(&frame.bytes);
        }
    }
    terminal.viewport(80, 24)
}

#[test]
fn replayed_frames_reproduce_the_daemons_own_screen() {
    let grid = replay_fidelity_frames();
    let daemon_screen = corpus_text("herdr-0.8.0/frame-fidelity/herdr-screen.txt");

    // herdr returns only the rows it has written; the grid is always the full viewport.
    // Trailing blank rows are agreement, not difference.
    let actual: Vec<String> =
        grid.rows.iter().map(|row| row.text().trim_end_matches(' ').to_string()).collect();

    for (index, want) in daemon_screen.split('\n').enumerate() {
        let want = want.trim_end_matches(' ');
        if want.is_empty() {
            continue;
        }
        let got = actual.get(index).map_or("<past end of grid>", String::as_str);
        assert_eq!(
            got, want,
            "\nRow {index} differs between the two renderers.\n  herdr:      {want:?}\n  \
             libghostty: {got:?}"
        );
    }
}

#[test]
fn wide_characters_occupy_two_cells_and_leave_a_spacer() {
    let grid = replay_fidelity_frames();
    let row = grid
        .rows
        .iter()
        .find(|row| row.text().starts_with("wide: "))
        .expect("the fidelity corpus should carry a `wide:` row");

    // A reader that treated 你 as one cell would still produce the right text, and would put
    // every following column in the wrong place. The spacer is what says otherwise.
    let han = row
        .cells
        .iter()
        .position(|cell| cell.text == "你")
        .expect("the `wide:` row should carry a wide character");
    assert_eq!(row.cells[han].width, Width::Wide);
    assert_eq!(row.cells[han + 1].width, Width::SpacerTail);
    assert!(row.cells[han + 1].text.is_empty());
}

#[test]
fn a_combining_mark_stays_in_the_cell_it_belongs_to() {
    let grid = replay_fidelity_frames();
    let row = grid
        .rows
        .iter()
        .find(|row| row.text().starts_with("combining: "))
        .expect("the fidelity corpus should carry a `combining:` row");

    // The payload writes the same letter twice: once decomposed as e + U+0301, once
    // precomposed as U+00E9. A reader that returned only a cell's first codepoint would drop
    // the mark from the first and render "e and é".
    //
    // Both forms are spelled by codepoint rather than typed as characters, because the
    // difference between them is the entire point and two literal `é` in a source file look
    // identical. The Swift version of this test compared against typed characters and passed
    // on Swift's canonical equivalence - so it would have passed just as well had the two
    // forms been swapped, or had the cell handed back a normalized string.
    let text = row.text();
    let expected = "e\u{301} and \u{e9}";
    assert!(text.contains(expected), "combining mark lost: {text:?}");
}
