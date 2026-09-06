//! Reading a tab's arrangement out of herdr's own tree. Cases and their reasoning live in
//! corpus/conformance/layout-export.json.

use conformance::{Conformance, fields};
use muster_core::names::{Mint, Names};
use muster_herdr::{read_exported_layout, read_layout};
use serde_json::{Value, json};

#[test]
fn layout_export_conformance() {
    let corpus = Conformance::load("layout-export.json");

    let ran = corpus.run(|given| {
        // Refusing is what most of these are about, so it is an answer rather than an error:
        // the driver reports that the tree did not read, and the corpus states which payloads
        // should end that way.
        let Some(layout) = read_exported_layout(given, &names()) else {
            return Ok(fields([("read", Some(json!(false)))]));
        };
        Ok(fields([
            ("read", Some(json!(true))),
            ("tab", Some(json!(layout.tab.to_string()))),
            // The core's own rendering rather than the driver's, so a case here and a case in
            // layout-reconstruction.json say the same thing about the same tree.
            ("tree", Some(json!(layout.root.to_string()))),
            ("focused", layout.focused.map(|pane| json!(pane.to_string()))),
            ("zoomed", layout.zoomed.map(|pane| json!(pane.to_string()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// Both readers agree about the tab they were both recorded describing.
///
/// The strongest thing available without a daemon: one session published this arrangement as
/// rectangles and as a tree, and the rectangles reader is already judged against herdr's own
/// export. So agreement here is two independent paths landing on one answer, rather than a
/// second copy of either one's code.
///
/// The two recordings are one moment, not two that resemble each other: the layout scenario asks
/// for `session.snapshot` and `layout.export` back to back with nothing in between. So the trees
/// are compared whole, ratios and all, which a pairing across two moments could not do - and a
/// reader that drops ratios is one of the ways to be wrong here.
#[test]
fn the_two_readers_describe_the_same_tab_the_same_way() {
    let flat = Conformance::load("layout-reconstruction.json");
    let flat = flat
        .cases
        .iter()
        .find(|case| case.name == "five panes at three levels")
        .expect("the rectangles corpus carries the deep case");
    let rebuilt = read_layout(&flat.given, &names()).expect("the rectangles case reads");

    let tree = Conformance::load("layout-export.json");
    let tree = tree
        .cases
        .iter()
        .find(|case| case.name == "five panes at three levels, stated rather than rebuilt")
        .expect("the export corpus carries the same arrangement");
    let read = read_exported_layout(&tree.given, &names()).expect("the exported case reads");

    assert_eq!(read.tab, rebuilt.tab, "the two recordings are of different tabs");
    assert_eq!(read.focused, rebuilt.focused);
    assert_eq!(
        read.root.to_string(),
        rebuilt.root.to_string(),
        "the rectangles and the tree describe different arrangements of one tab.\n  Impact: one \
         of the two readers is wrong, so a tab renders differently depending on which verb \
         described it.\n  rectangles: {}\n  tree:       {}",
        rebuilt.root,
        read.root,
    );
}

/// The recordings, not hand-made copies of them.
///
/// The point of the recorded cases is that they are what a real daemon published, so this
/// checks them back against the transcript they came out of. A case that drifted into what
/// somebody believed herdr answers would otherwise pass forever.
#[test]
fn the_recorded_cases_are_what_herdr_answered() {
    let corpus = Conformance::load("layout-export.json");
    let published: Vec<Value> =
        std::fs::read_to_string(corpus_path("herdr-0.8.0/layout/wire.ndjson"))
            .expect("the layout transcript is checked in")
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|record| record["dir"] == "in")
            .filter_map(|record| {
                record["result"]["layout"].as_object().map(|_| record["result"]["layout"].clone())
            })
            .collect();

    for name in [
        "a divider drag answers with the arrangement it settled on",
        "five panes at three levels, stated rather than rebuilt",
        "sixteen panes at every depth the recording reached",
    ] {
        let case = corpus
            .cases
            .iter()
            .find(|case| case.name == name)
            .unwrap_or_else(|| panic!("the corpus carries {name:?}"));
        assert!(
            published.contains(&case.given),
            "{name:?} is not an arrangement herdr was recorded publishing"
        );
    }
}

fn corpus_path(relative: &str) -> std::path::PathBuf {
    let mut directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("corpus").join(relative);
        if candidate.exists() {
            return candidate;
        }
        directory = directory.parent().expect("the corpus sits above this crate");
    }
}

/// A registry whose name for a pane is the daemon's own id for it.
///
/// So a case here says `w1:p1` and is about the reading rather than about the mint, which has
/// cases of its own in `corpus/conformance/pane-names.json`.
fn names() -> Names {
    Names::alone("local", Mint::Backend)
}
