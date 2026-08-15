//! What exists, as opposed to what is on screen. Cases and their reasoning live in
//! corpus/conformance/roster.json.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, PaneKey};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use muster_core::roster::{Roster, RosterPane};
use serde_json::{Value, json};
use support::backend::{read_snapshot, text};

#[test]
fn roster_conformance() {
    let corpus = Conformance::load("roster.json");

    let ran = corpus.run(|given| {
        // Each daemon's world, or none for one attached whose subscription has not
        // bootstrapped - which is a state a window really passes through.
        let mut worlds: BTreeMap<DaemonId, Mirror> = BTreeMap::new();
        let mut composition = Composition::new();

        for described in given.get("daemons").and_then(Value::as_array).into_iter().flatten() {
            let id = DaemonId::new(text(described, "id"));
            composition.attach_daemon(Daemon {
                id: id.clone(),
                endpoint: Endpoint::Local { socket_path: None },
            });
            let Some(name) = described.get("world").and_then(Value::as_str) else { continue };
            let snapshot =
                given.get("worlds").and_then(|worlds| worlds.get(name)).ok_or_else(|| {
                    CaseError::new(format!("the case names a world `{name}` it does not describe"))
                })?;
            let mut mirror = Mirror::new();
            mirror.bootstrap(read_snapshot(snapshot));
            worlds.insert(id, mirror);
        }

        for region in given.get("regions").and_then(Value::as_array).into_iter().flatten() {
            composition.open_region(
                &DaemonId::new(text(region, "daemon")),
                WorkspaceId::new(text(region, "workspace")),
                TabId::new(text(region, "tab")),
            );
        }

        let showing = read_showing(given)?;
        let roster = Roster::of(&composition, |daemon| worlds.get(daemon), &showing);
        Ok(fields([(
            "panes",
            Some(json!(roster.panes.iter().map(describe).collect::<Vec<String>>())),
        )]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// One row, as a line.
///
/// A line rather than an object because order is most of what these cases are about, and a
/// list of readable lines shows a wrong order at a glance where a list of objects hides it.
fn describe(pane: &RosterPane) -> String {
    format!(
        "{} tab={} label={:?} {}",
        pane.key,
        pane.tab,
        pane.label,
        if pane.on_screen { "on-screen" } else { "hidden" }
    )
}

fn read_showing(given: &Value) -> Result<BTreeSet<PaneKey>, CaseError> {
    given
        .get("showing")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| {
            let text = entry
                .as_str()
                .ok_or_else(|| CaseError::new("`showing` holds something that is not a pane"))?;
            let (daemon, pane) = text.split_once('/').ok_or_else(|| {
                CaseError::new(format!("`{text}` names no daemon - write a pane `local/w1:p1`"))
            })?;
            Ok(PaneKey { daemon: DaemonId::new(daemon), pane: PaneId::new(pane) })
        })
        .collect()
}
