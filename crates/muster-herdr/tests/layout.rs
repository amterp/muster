//! Rebuilding a tab's tree out of herdr's rectangles. Cases and their reasoning live in
//! corpus/conformance/layout-reconstruction.json.

use conformance::{Conformance, fields};
use muster_core::names::{Mint, Names};
use muster_herdr::read_layout;
use serde_json::{Value, json};

#[test]
fn layout_reconstruction_conformance() {
    let corpus = Conformance::load("layout-reconstruction.json");

    let ran = corpus.run(|given| {
        // A tab whose arrangement will not read is the case several of these are about, so
        // it is an answer rather than an error: the driver reports that it did not read,
        // and the corpus states which inputs should end that way.
        let Some(layout) = read_layout(given, &names()) else {
            return Ok(fields([("read", Some(json!(false)))]));
        };
        Ok(fields([
            ("tab", Some(json!(layout.tab.to_string()))),
            // One line, from the core's own rendering rather than the driver's, so that a
            // case says what a run log would say about the same tree.
            ("tree", Some(json!(layout.root.to_string()))),
            ("focused", layout.focused.map(|pane| json!(pane.to_string()))),
            ("zoomed", layout.zoomed.map(|pane| json!(pane.to_string()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// The recording, not a hand-made copy of it.
///
/// The point of the deep cases is that they are what a real daemon published, so reading
/// them from the transcript is what stops them drifting into what somebody believed it
/// published. A case that names a missing file fails loudly rather than passing on nothing.
#[test]
fn the_deep_cases_are_the_recorded_layout() {
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(corpus_path("herdr-0.8.0/layout/deep-session.snapshot.json"))
            .expect("the layout recording is checked in"),
    )
    .expect("the recording is JSON");
    let recorded = recorded["layouts"][0].clone();

    let corpus = Conformance::load("layout-reconstruction.json");
    let case = corpus
        .cases
        .iter()
        .find(|case| case.name == "five panes at three levels")
        .expect("the corpus carries the deep case");

    assert_eq!(case.given, recorded, "the deep case is not the layout that was recorded");
}

fn corpus_path(relative: &str) -> std::path::PathBuf {
    let mut directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("corpus").join(relative);
        if candidate.exists() {
            return candidate;
        }
        directory = directory.parent().expect("a corpus directory above this crate");
    }
}

/// A registry whose name for a pane is the daemon's own id for it.
///
/// So a case here says `w1:p1` and is about the reading rather than about the mint, which has
/// cases of its own in `corpus/conformance/pane-names.json`.
fn names() -> Names {
    Names::alone("local", Mint::Backend)
}
