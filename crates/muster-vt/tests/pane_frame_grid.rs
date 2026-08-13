//! What a user would see, computed by the engine that would show it to them.
//!
//! These replay bytes a real herdr daemon actually sent (`corpus/herdr-0.8.0/frames/`)
//! through libghostty-vt and snapshot the resulting grid. The oracle is the screen, not a
//! substring of the stream and not a pixel: `docs/testing.md`, "assert what the user sees".

mod support;

use muster_herdr::{FrameDecoder, PaneStreamEvent};
use muster_vt::Terminal;
use support::{corpus_file, expect_snapshot};

#[test]
fn a_recorded_attach_repaint_renders_the_panes_screen() {
    // 35 KB of absolute positioning and per-cell SGR: what herdr sends a client that has
    // just attached, and the one frame every surface is guaranteed to see.
    let ansi = corpus_file("herdr-0.8.0/frames/frame-001-attach.ansi");

    let terminal = Terminal::new(80, 24).expect("libghostty-vt should give us a terminal");
    terminal.write(&ansi);

    expect_snapshot(&terminal.viewport(80, 24).render(), "attach-repaint.txt");
}

#[test]
fn the_whole_recorded_stream_converges_on_the_same_screen() {
    // Through the real decoder, so the test covers the path a pane actually takes:
    // envelopes off the wire, frames out, bytes into a terminal.
    let stream = corpus_file("herdr-0.8.0/frames/frames.ndjson");
    let mut decoder = FrameDecoder::new();
    let terminal = Terminal::new(80, 24).expect("libghostty-vt should give us a terminal");

    let mut frames = 0;
    for event in decoder.consume(&stream) {
        if let PaneStreamEvent::Frame(frame) = event {
            terminal.write(&frame.bytes);
            frames += 1;
        }
    }

    assert!(frames > 0, "the corpus stream carried no frames, so nothing was rendered");
    expect_snapshot(&terminal.viewport(80, 24).render(), "recorded-stream.txt");
}

#[test]
fn a_full_repaint_replaces_the_screen_rather_than_layering_onto_it() {
    // A surface attaching to a live pane starts mid-stream and must not inherit whatever was
    // on it (architecture.md, "the shell owns nothing"). herdr's repaint clears first; this
    // pins that we get the clear, not a merge.
    let terminal = Terminal::new(20, 3).expect("libghostty-vt should give us a terminal");
    terminal.write(b"stale text everywhere");
    terminal.write(&corpus_file("herdr-0.8.0/frames/frame-001-attach.ansi"));

    let rows = terminal.viewport(20, 3).rows;
    assert!(!rows.iter().any(|row| row.text().contains("stale")));
}
