//! Reopening a window onto a session that moved on. Cases live in
//! corpus/conformance/composition-saved.json.
//!
//! The round trip is asserted separately below, because a corpus of restore rules proves
//! nothing if the file the rules run against cannot be read back.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::presentation::{FontSizes, Frame, Presentation};
use muster_core::composition::record::{Composition, Daemon, DaemonId, Endpoint, PaneKey};
use muster_core::composition::saved::{Saved, SavedRegion, from_toml, to_toml};
use muster_core::mirror::backend::{PaneId, TabId};
use serde_json::{Value, json};

#[test]
fn composition_saved_conformance() {
    let corpus = Conformance::load("composition-saved.json");

    let ran = corpus.run(|given| {
        let saved = saved(given)?;
        let held = held(given)?;
        let restorable = saved.restorable(|daemon, tab| held.contains(&key(daemon, tab)));

        Ok(fields([
            (
                "regions",
                Some(json!(
                    restorable
                        .regions
                        .iter()
                        // Whole numbers, because a weight in a case is there to be followed
                        // through the list rather than to test float rendering.
                        .map(|region| format!(
                            "{}/{}@{:.0}",
                            region.daemon, region.tab, region.weight
                        ))
                        .collect::<Vec<String>>()
                )),
            ),
            ("focused", Some(restorable.focused.map_or(Value::Null, |place| json!(place)))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn what_is_written_is_what_comes_back() {
    // The file is the only thing between one run and the next, so a field that writes and
    // does not read is an arrangement that quietly loses something every restart. Both
    // endpoint shapes, because ssh carries three fields nothing else does.
    let mut composition = Composition::new();
    composition.attach_daemon(Daemon {
        id: DaemonId::new("local"),
        endpoint: Endpoint::Local { socket_path: None },
    });
    composition.attach_daemon(Daemon {
        id: DaemonId::new("devenv"),
        endpoint: Endpoint::Ssh {
            host: "devenv".to_string(),
            options: vec!["-p".to_string(), "2222".to_string()],
            socket_path: Some("/run/herdr.sock".to_string()),
        },
    });
    let region = composition
        .open_region(&DaemonId::new("local"), TabId::new("w1:t1"))
        .expect("the daemon was just attached");
    composition.focus_pane(region, PaneId::new("w1:p1"));
    composition
        .open_region(&DaemonId::new("devenv"), TabId::new("w1:t2"))
        .expect("the daemon was just attached");

    // Nothing at its default, so a round trip that quietly dropped any of it would still fail.
    // The frame carries fractions, because a window dragged half a point is what a trackpad
    // produces and a rectangle rounded on the way through comes back a pixel out every launch.
    // Two panes on two daemons, because a text size is keyed by both and a file that wrote
    // only the pane would hand one machine's size to the other's pane of the same name.
    let sizes: FontSizes = [
        (PaneKey::new(&DaemonId::new("local"), &PaneId::new("w1:p1")), 3),
        (PaneKey::new(&DaemonId::new("devenv"), &PaneId::new("w1:p1")), -2),
    ]
    .into_iter()
    .collect();

    let written = Saved::of(
        &composition,
        Presentation::default()
            .with_sidebar(false)
            .with_frame(Some(Frame { x: -120.5, y: 240.0, width: 1400.0, height: 902.5 }), true),
        &sizes,
    );
    let read = from_toml(&to_toml(&written)).expect("what this wrote, it can read");

    assert_eq!(read, written, "the file lost something between writing and reading it");
}

/// A window that has never settled anywhere writes no rectangle, and reads back as one.
///
/// The absence has to survive as an absence: a frame that came back as zeroes would be a window
/// opening at the corner of a display on its second launch, which is a worse first impression
/// than the centred default it is meant to replace.
#[test]
fn a_window_with_no_frame_says_so_rather_than_writing_a_corner() {
    let file = to_toml(&Saved::default());
    assert!(
        !file.contains("width"),
        "a window that has never settled wrote a rectangle anyway:\n{file}"
    );

    let read = from_toml(&file).expect("what this wrote, it can read");
    assert_eq!(read.presentation.frame, None);
    assert!(!read.presentation.full_screen);
}

/// Half a rectangle is no rectangle.
///
/// Only a hand-edit produces this, and the answer is the one every other refusal in this file
/// reaches: open where a first launch would. Refusing the file outright would lose the
/// arrangement - the tabs, their order, their widths - over four numbers the window can do
/// without.
#[test]
fn a_partly_written_rectangle_is_ignored_rather_than_half_applied() {
    let whole = to_toml(&Saved {
        presentation: Presentation::default()
            .with_frame(Some(Frame { x: 10.0, y: 20.0, width: 800.0, height: 600.0 }), false),
        ..Saved::default()
    });

    for missing in ["x = 10.0\n", "height = 600.0\n"] {
        let file = whole.replace(missing, "");
        assert_ne!(file, whole, "the round trip stopped writing {missing:?}");
        let read = from_toml(&file).expect("a partial rectangle is not an unreadable file");
        assert_eq!(read.presentation.frame, None, "half a rectangle came back as a whole one");
    }

    // And a size nobody can see or grab, which is the other way a hand-edit goes wrong.
    let file = whole.replace("width = 800.0", "width = 0.0");
    let read = from_toml(&file).expect("a zero width is not an unreadable file");
    assert_eq!(read.presentation.frame, None);
}

/// A font size nobody could have pressed their way to comes back as one they could.
///
/// The state file is Muster's to write and a person's to read, so a number that arrived by hand
/// is not a state to refuse - it is one to bring back inside the range, the same way the setter
/// does when a key is held down. Refusing would cost the whole arrangement over a font size.
#[test]
fn a_hand_edited_font_size_is_brought_back_inside_the_range() {
    let pane = PaneKey::new(&DaemonId::new("local"), &PaneId::new("w1:p1"));
    let file = to_toml(&Saved {
        font_sizes: [(pane.clone(), 3)].into_iter().collect(),
        ..Saved::default()
    })
    .replace("font_size_offset = 3", "font_size_offset = 100000");

    let read = from_toml(&file).expect("an out-of-range offset is not an unreadable file");
    assert_eq!(read.font_sizes.offset(&pane), FontSizes::LIMIT);
}

/// A window nobody has sized writes no pane rows at all.
///
/// The `[window]` keys are written even at their default, so a person opening the file learns
/// they exist. This is a list of exceptions rather than a fixed set, and a row per pane saying
/// "the configured size" would be a table that grows with the window and says nothing.
#[test]
fn a_window_nobody_has_sized_writes_no_pane_rows() {
    let file = to_toml(&Saved::default());
    assert!(!file.contains("[[pane]]"), "an unsized window wrote pane rows anyway:\n{file}");
    assert!(
        !file.contains("font_size_offset"),
        "an unsized window wrote a text size anyway:\n{file}"
    );
}

/// A file from when text was sized for the whole window loses that size and keeps everything
/// else.
///
/// There is nowhere to put it: the key named no pane, and the panes it applied to are not
/// recoverable from a file that never listed them. Losing it costs one relaunch at the
/// configured size; refusing the file would cost the arrangement, which is much worse and
/// which the version is reserved for.
#[test]
fn a_window_wide_text_size_is_dropped_and_the_rest_survives() {
    let file = to_toml(&Saved {
        presentation: Presentation::default().with_sidebar(false),
        ..Saved::default()
    })
    .replace("[window]", "[window]\nfont_size_offset = 4");

    let read = from_toml(&file).expect("an old key is not an unreadable file");
    assert!(!read.presentation.sidebar, "the rest of the window was lost with the old key");
    assert_eq!(read.font_sizes, FontSizes::default(), "the window-wide size came back as a pane");
}

#[test]
fn a_file_from_a_format_nobody_knows_is_refused_by_name() {
    // Refused rather than partially read: the cost of ignoring it is a window that opens as a
    // first launch does, and the cost of guessing is a window that opens wrong.
    let refusal = from_toml("version = 99\n").expect_err("version 99 is not this format");
    assert!(
        refusal.contains("version 99") && refusal.contains("first launch"),
        "the refusal should name the version it found and what happens next, and said: \
         {refusal}"
    );

    let refusal = from_toml("regions = []\n").expect_err("a file with no version is not readable");
    assert!(refusal.contains("version"), "the refusal should say what is missing: {refusal}");
}

/// One case's `given`, as the arrangement it describes.
fn saved(given: &Value) -> Result<Saved, CaseError> {
    let regions = given
        .get("regions")
        .and_then(Value::as_array)
        .ok_or_else(|| CaseError::new("`regions` is missing: there is nothing to restore"))?;
    Ok(Saved {
        daemons: Vec::new(),
        regions: regions
            .iter()
            .map(|region| SavedRegion {
                daemon: DaemonId::new(region["daemon"].as_str().unwrap_or_default()),
                tab: TabId::new(region["tab"].as_str().unwrap_or_default()),
                weight: serde_json::from_value(region["weight"].clone()).unwrap_or(1.0),
                pane: None,
            })
            .collect(),
        focused: given
            .get("focused")
            .and_then(Value::as_u64)
            .and_then(|place| usize::try_from(place).ok()),
        // Not what these cases are about: they judge which regions survive a check against
        // the daemons, and nothing here is checked against anything.
        presentation: Presentation::default(),
        font_sizes: FontSizes::default(),
    })
}

/// The tabs the daemons turn out to hold, spelled `<daemon>/<tab>`.
fn held(given: &Value) -> Result<BTreeSet<String>, CaseError> {
    let held = given
        .get("held")
        .and_then(Value::as_array)
        .ok_or_else(|| CaseError::new("`held` is missing: nothing says what still exists"))?;
    Ok(held.iter().filter_map(|entry| entry.as_str().map(str::to_string)).collect())
}

fn key(daemon: &DaemonId, tab: &TabId) -> String {
    format!("{daemon}/{tab}")
}
