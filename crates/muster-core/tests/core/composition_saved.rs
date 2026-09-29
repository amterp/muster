//! Reopening a window onto a session that moved on. Cases live in
//! corpus/conformance/composition-saved.json.
//!
//! The round trip is asserted separately below, because a corpus of restore rules proves
//! nothing if the file the rules run against cannot be read back.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::presentation::{FontSizes, Frame, Presentation};
use muster_core::composition::record::{Composition, Daemon, DaemonId, Endpoint, PaneKey};
use muster_core::composition::saved::{Saved, SavedRegion, SavedTab, from_toml, to_toml};
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
                "tabs",
                Some(json!(
                    restorable
                        .tabs
                        .iter()
                        // Whole numbers, because a weight in a case is there to be followed
                        // through the list rather than to test float rendering. A star marks
                        // the region the keyboard was in.
                        .map(|tab| {
                            let regions: Vec<String> = tab
                                .regions
                                .iter()
                                .map(|region| {
                                    format!(
                                        "{}@{:.0}{}",
                                        region.daemon,
                                        region.weight,
                                        if region.keyboard { "*" } else { "" }
                                    )
                                })
                                .collect();
                            format!("{} {}", tab.id, regions.join(" "))
                        })
                        .collect::<Vec<String>>()
                )),
            ),
            ("showing", Some(restorable.showing.map_or(Value::Null, |tab| json!(tab.as_str())))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// A daemon that answers after the window has opened. Cases live in
/// corpus/conformance/composition-saved-late.json.
#[test]
fn composition_saved_late_conformance() {
    let corpus = Conformance::load("composition-saved-late.json");

    let ran = corpus.run(|given| {
        let left = saved_from(given, "left")?;
        let late: BTreeSet<DaemonId> = given
            .get("late")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                CaseError::new("`late` is missing: nothing says which daemons are late")
            })?
            .iter()
            .filter_map(Value::as_str)
            .map(DaemonId::new)
            .collect();
        let mut now = composition_of(given, "now")?;
        let kept =
            Saved::of(&now, Presentation::default(), &FontSizes::default()).keeping(&left, &late);

        // The late daemons answer: each opens its regions where a reconcile would, at the end,
        // with the widths the file had, and the window is then put in the order the file says.
        for daemon in &late {
            now.attach_daemon(Daemon {
                id: daemon.clone(),
                endpoint: Endpoint::Local { socket_path: None },
            });
        }
        for tab in &left.tabs {
            for region in tab.regions.iter().filter(|region| late.contains(&region.daemon)) {
                if let Some(id) = now.open_region(&region.daemon, tab.id.clone()) {
                    now.set_weight(id, region.weight);
                }
            }
        }
        let wanted =
            Saved::of(&now, Presentation::default(), &FontSizes::default()).keeping(&left, &late);
        now.arrange_like(&wanted);
        let arrived = Saved::of(&now, Presentation::default(), &FontSizes::default());

        Ok(fields([
            ("kept", Some(json!(described(&kept.tabs)))),
            ("arrived", Some(json!(described(&arrived.tabs)))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// Tabs as a case spells them: `t1 local@2*`, a star on the region the keyboard was in.
fn described(tabs: &[SavedTab]) -> Vec<String> {
    tabs.iter()
        .map(|tab| {
            let regions: Vec<String> = tab
                .regions
                .iter()
                .map(|region| {
                    format!(
                        "{}@{:.0}{}",
                        region.daemon,
                        region.weight,
                        if region.keyboard { "*" } else { "" }
                    )
                })
                .collect();
            format!("{} {}", tab.id, regions.join(" "))
        })
        .collect()
}

/// A composition holding the regions a case lists under `key`, in that order.
fn composition_of(given: &Value, key: &str) -> Result<Composition, CaseError> {
    let listed = saved_from(given, key)?;
    let mut composition = Composition::new();
    for tab in &listed.tabs {
        for region in &tab.regions {
            composition.attach_daemon(Daemon {
                id: region.daemon.clone(),
                endpoint: Endpoint::Local { socket_path: None },
            });
            let id = composition
                .open_region(&region.daemon, tab.id.clone())
                .ok_or_else(|| CaseError::new("a region could not be opened"))?;
            composition.set_weight(id, region.weight);
            if region.keyboard {
                composition.focus_region(id);
            }
        }
    }
    Ok(composition)
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
            socket_path: Some("/run/daemon.sock".to_string()),
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
            .with_sidebar_width(262.5)
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
        !file.lines().any(|line| line.trim_start().starts_with("width")),
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

/// An arrangement a Muster on herdr wrote keeps what no daemon has an opinion on, and nothing
/// that named what herdr held.
///
/// Its tabs and panes were herdr's, and muster-daemon holds none of them, so read as they stand
/// every region would fail its check. The window's frame and list are the window's own, and a
/// machine Muster started its own daemon on is the same machine with the new daemon on it; a
/// daemon reached by a socket the file named was a herdr, and is left out.
#[test]
fn an_arrangement_from_a_muster_on_herdr_keeps_its_window_and_its_machines() {
    for version in [3, 4] {
        let saved = from_toml(&format!(
            "version = {version}\n\
             showing = \"t1w3r07bsd\"\n\
             [window]\n\
             sidebar = false\n\
             full_screen = true\n\
             [[daemon]]\n\
             id = \"local\"\n\
             [[daemon]]\n\
             id = \"devenv\"\n\
             ssh = \"devenv\"\n\
             [[daemon]]\n\
             id = \"theirs\"\n\
             ssh = \"box\"\n\
             socket = \"/home/them/.config/herdr/herdr.sock\"\n\
             [[region]]\n\
             daemon = \"local\"\n\
             tab = \"t1w3r07bsd\"\n\
             [[pane]]\n\
             daemon = \"local\"\n\
             pane = \"p1w3r07bsd\"\n\
             font_size_offset = 2\n"
        ))
        .expect("a file from herdr is read for what still holds");
        assert!(saved.tabs.is_empty() && saved.showing.is_none(), "version {version}: {saved:?}");
        assert_eq!(saved.font_sizes, FontSizes::default(), "herdr's panes are not these");
        assert!(!saved.presentation.sidebar && saved.presentation.full_screen);
        let kept: Vec<&str> = saved.daemons.iter().map(|daemon| daemon.id.as_str()).collect();
        assert_eq!(kept, ["local", "devenv"], "the daemon behind a named socket was a herdr");
    }
}

/// One case's `given`, as the arrangement it describes.
///
/// The rows are flat and each names its tab, the way the file itself spells them, and the tabs
/// come out in the order their first row appears.
fn saved(given: &Value) -> Result<Saved, CaseError> {
    saved_from(given, "regions")
}

fn saved_from(given: &Value, key: &str) -> Result<Saved, CaseError> {
    let regions = given.get(key).and_then(Value::as_array).ok_or_else(|| {
        CaseError::new(format!("`{key}` is missing: there is nothing to restore"))
    })?;
    let mut tabs: Vec<SavedTab> = Vec::new();
    for region in regions {
        let id = TabId::new(region["tab"].as_str().unwrap_or_default());
        let held = SavedRegion {
            daemon: DaemonId::new(region["daemon"].as_str().unwrap_or_default()),
            weight: serde_json::from_value(region["weight"].clone()).unwrap_or(1.0),
            pane: None,
            keyboard: region["keyboard"].as_bool().unwrap_or_default(),
        };
        match tabs.iter_mut().find(|tab| tab.id == id) {
            Some(tab) => tab.regions.push(held),
            None => tabs.push(SavedTab { id, regions: vec![held] }),
        }
    }
    Ok(Saved {
        daemons: Vec::new(),
        tabs,
        showing: given.get("showing").and_then(Value::as_str).map(TabId::new),
        // Not what these cases are about: they judge which tabs survive a check against the
        // daemons, and nothing here is checked against anything.
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
