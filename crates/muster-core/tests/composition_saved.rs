//! Reopening a window onto a session that moved on. Cases live in
//! corpus/conformance/composition-saved.json.
//!
//! The round trip is asserted separately below, because a corpus of restore rules proves
//! nothing if the file the rules run against cannot be read back.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::record::{Composition, Daemon, DaemonId, Endpoint};
use muster_core::composition::saved::{Saved, SavedRegion, from_toml, to_toml};
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
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
        .open_region(&DaemonId::new("local"), WorkspaceId::new("w1"), TabId::new("w1:t1"))
        .expect("the daemon was just attached");
    composition.focus_pane(region, PaneId::new("w1:p1"));
    composition
        .open_region(&DaemonId::new("devenv"), WorkspaceId::new("w1"), TabId::new("w1:t2"))
        .expect("the daemon was just attached");

    let written = Saved::of(&composition);
    let read = from_toml(&to_toml(&written)).expect("what this wrote, it can read");

    assert_eq!(read, written, "the file lost something between writing and reading it");
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
                workspace: WorkspaceId::new("w1"),
                tab: TabId::new(region["tab"].as_str().unwrap_or_default()),
                weight: serde_json::from_value(region["weight"].clone()).unwrap_or(1.0),
                pane: None,
            })
            .collect(),
        focused: given
            .get("focused")
            .and_then(Value::as_u64)
            .and_then(|place| usize::try_from(place).ok()),
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
