//! What exists, as opposed to what is on screen. Cases and their reasoning live in
//! corpus/conformance/roster.json.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, PaneKey, TabKey};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use muster_core::roster::{Roster, RosterPane, RosterTab, TabStep};
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
        Ok(fields([
            (
                "stepped",
                read_step(given)?.map(|(from, direction)| {
                    json!(roster.step(from.as_ref(), direction).map(|tab| tab.key.to_string()))
                }),
            ),
            (
                "at",
                given
                    .get("at")
                    .and_then(Value::as_u64)
                    .and_then(|place| usize::try_from(place).ok())
                    .map(|place| json!(roster.at(place).map(|tab| tab.key.to_string()))),
            ),
            ("tabs", Some(json!(roster.tabs().map(describe_tab).collect::<Vec<String>>()))),
            (
                "panes",
                Some(json!(
                    roster
                        .tabs()
                        .flat_map(|tab| tab.panes.iter().map(move |pane| describe_pane(tab, pane)))
                        .collect::<Vec<String>>()
                )),
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// One tab, as a line.
///
/// A line rather than an object because order and numbering are most of what these cases are
/// about, and a list of readable lines shows a wrong order at a glance where a list of objects
/// hides it.
fn describe_tab(tab: &RosterTab) -> String {
    format!(
        "{} place={} label={:?}{} {}",
        tab.key,
        tab.place,
        tab.label,
        described("given-name", tab.given_name.as_deref()),
        if tab.on_screen { "on-screen" } else { "hidden" }
    )
}

/// An optional part of a line, printed only when it says something.
///
/// Most rows have neither a name somebody typed nor a second line, so printing both always
/// would put `given-name="" subtitle=""` on every line of every case here and bury what each
/// one is actually about.
fn described(key: &str, value: Option<&str>) -> String {
    value.map(|value| format!(" {key}={value:?}")).unwrap_or_default()
}

/// One pane, as a line, with the tab it sits under.
///
/// The tab is printed even though the nesting already says it, so that a case can be read
/// without counting rows back up to the heading it belongs to.
fn describe_pane(tab: &RosterTab, pane: &RosterPane) -> String {
    format!(
        "{} tab={} label={:?}{}{} {}",
        pane.key,
        tab.key.tab,
        pane.label,
        described("subtitle", pane.subtitle.as_deref()),
        described("given-name", pane.given_name.as_deref()),
        if pane.on_screen { "on-screen" } else { "hidden" }
    )
}

/// The step a case asks for, or none for a case that is only about the list.
///
/// `from` is named outright rather than taken from whichever region has focus, so that a case
/// can start from a tab nothing is showing - which is the interesting half, since the whole
/// point of stepping tabs is reaching the ones no region has.
fn read_step(given: &Value) -> Result<Option<(Option<TabKey>, TabStep)>, CaseError> {
    let Some(step) = given.get("step") else { return Ok(None) };
    let named = text(step, "direction");
    let direction = TabStep::parse(&named).ok_or_else(|| {
        CaseError::new(format!("`{named}` is not a tab step - write `next` or `previous`"))
    })?;
    let from = match step.get("from").and_then(Value::as_str) {
        Some(text) => Some(read_tab(text)?),
        None => None,
    };
    Ok(Some((from, direction)))
}

fn read_tab(text: &str) -> Result<TabKey, CaseError> {
    let (daemon, tab) = text.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{text}` names no daemon - write a tab `local/w1:t1`"))
    })?;
    Ok(TabKey { daemon: DaemonId::new(daemon), tab: TabId::new(tab) })
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
