//! What exists, as opposed to what is on screen. Cases and their reasoning live in
//! corpus/conformance/roster.json.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, PaneKey};
use muster_core::input::NumberedChords;
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::roster::{Landing, Numbering, Roster, RosterPane, RosterTab, TabStep};
use serde_json::{Value, json};
use support::backend::{read_snapshot, text};

#[test]
fn roster_conformance() {
    let corpus = Conformance::load("roster.json");

    let ran = corpus.run(|given| {
        // Each daemon's world, or none for one attached whose subscription has not
        // bootstrapped - which is a state a window really passes through.
        let mut worlds: BTreeMap<DaemonId, Mirror> = BTreeMap::new();
        let mut attached: Vec<DaemonId> = Vec::new();
        let mut composition = Composition::new();

        for described in given.get("daemons").and_then(Value::as_array).into_iter().flatten() {
            let id = DaemonId::new(text(described, "id"));
            attached.push(id.clone());
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

        // The window's tabs come from what its daemons hold, in the order the case attaches
        // them - which is what a launch with nothing saved produces.
        for id in &attached {
            if let Some(mirror) = worlds.get(id) {
                composition.reconcile(id, mirror);
            }
        }
        // `regions` then says which machines share a tab, which is the one thing a reconcile
        // cannot produce on its own: grouping is Muster's, and no daemon knows the other exists.
        for region in given.get("regions").and_then(Value::as_array).into_iter().flatten() {
            composition.open_region(
                &DaemonId::new(text(region, "daemon")),
                TabId::new(text(region, "tab")),
            );
        }
        // Which tab is on screen, when a case cares. Without one the window shows the first it
        // holds, which is what a launch onto a machine already holding tabs does.
        if let Some(tab) = given.get("showingTab").and_then(Value::as_str) {
            composition.show(&TabId::new(tab));
        }

        let showing = read_showing(given)?;
        let roster = Roster::of(&composition, |daemon| worlds.get(daemon), &showing);
        Ok(fields([
            (
                "stepped",
                read_step(given)?.map(|(from, direction)| {
                    json!(roster.step(from.as_ref(), direction).map(|tab| tab.id.to_string()))
                }),
            ),
            (
                "at",
                given
                    .get("at")
                    .and_then(Value::as_u64)
                    .and_then(|place| usize::try_from(place).ok())
                    .map(|place| json!(roster.at(place).map(|pane| pane.key.to_string()))),
            ),
            ("pressed", read_presses(given, &roster)?),
            ("tabs", Some(json!(roster.tabs().map(describe_tab).collect::<Vec<String>>()))),
            // The machines, which the tabs no longer group by. Only the ones with something to
            // say: a machine that is connected and holding panes says it through its panes.
            (
                "machines",
                Some(json!(
                    roster
                        .machines
                        .iter()
                        .map(|machine| format!(
                            "{} {} {} panes",
                            machine.id,
                            machine.health.as_str(),
                            machine.panes
                        ))
                        .collect::<Vec<String>>()
                )),
            ),
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

#[test]
fn every_numbering_scheme_is_pressed_in_the_corpus() {
    // A scheme added to the config and not pressed here is a control scheme nothing decides.
    // Both of these fail invisibly: the settled one would stop being pinned the moment the
    // prototype's cases outnumbered it, and the prototype has no other test of what a second
    // press means.
    let corpus = Conformance::load("roster.json");
    let pressed: Vec<String> = corpus
        .cases
        .iter()
        .filter_map(|case| case.given.get("numbered"))
        .map(|asked| text(asked, "scheme"))
        .collect();

    for scheme in NumberedChords::READABLE {
        assert!(
            pressed.iter().any(|named| named == scheme),
            "no corpus case presses a chord under `{scheme}`, so nothing pins what one does"
        );
    }
}

/// One tab, as a line.
///
/// A line rather than an object because order and numbering are most of what these cases are
/// about, and a list of readable lines shows a wrong order at a glance where a list of objects
/// hides it.
fn describe_tab(tab: &RosterTab) -> String {
    let daemons: Vec<String> = tab.daemons.iter().map(ToString::to_string).collect();
    format!(
        "{} on={} place={} label={:?}{} {}",
        tab.id,
        daemons.join("+"),
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
        "{} place={} tab={} label={:?}{}{} {}",
        pane.key,
        pane.place,
        tab.id,
        pane.label,
        described("subtitle", pane.subtitle.as_deref()),
        described("given-name", pane.given_name.as_deref()),
        if pane.on_screen { "on-screen" } else { "hidden" }
    )
}

/// The numbered chords a case presses, and what each one reached.
///
/// A sequence rather than one press, because under `tab_then_pane` a press means one thing or
/// another depending on what the press before it did - so a case pressing once could only ever
/// pin half the scheme. The line for each press says what it landed on *and* what the chords
/// name afterwards, which is the pair a person driving this has to be able to predict.
///
/// Run through the same [`Numbering::of`] and [`Landing::named`] the window runs on. A driver
/// that tracked the armed tab its own way would be a second implementation of the one thing
/// these cases exist to pin.
fn read_presses(given: &Value, roster: &Roster) -> Result<Option<Value>, CaseError> {
    let Some(asked) = given.get("numbered") else { return Ok(None) };
    let spelled = text(asked, "scheme");
    let scheme = NumberedChords::parse(&spelled).ok_or_else(|| {
        CaseError::new(format!(
            "`{spelled}` is not a numbering scheme - write `panes` or `tab_then_pane`"
        ))
    })?;

    let mut named = None;
    let mut landed = Vec::new();
    for press in asked.get("press").and_then(Value::as_array).into_iter().flatten() {
        let place = press
            .as_u64()
            .and_then(|place| usize::try_from(place).ok())
            .ok_or_else(|| CaseError::new("`press` holds something that is not a place"))?;
        let numbering = Numbering::of(scheme, named.as_ref(), roster);
        let landing = roster.numbered(&numbering, place);
        landed.push(describe_press(place, &numbering, landing.as_ref()));
        named = landing.and_then(|landing| landing.named());
    }
    Ok(Some(json!(landed)))
}

/// One press, as a line: what was being counted, what it reached, and what it left armed.
fn describe_press(place: usize, numbering: &Numbering, landing: Option<&Landing<'_>>) -> String {
    let counting = match numbering {
        Numbering::Panes => "panes".to_string(),
        Numbering::Tabs => "tabs".to_string(),
        Numbering::PanesIn(tab) => format!("panes in {tab}"),
    };
    match landing {
        Some(Landing::Pane(pane)) => format!("⌘{place} of {counting} → pane {}", pane.key),
        Some(Landing::Tab(tab, pane)) => {
            format!("⌘{place} of {counting} → tab {} landing on {}", tab.id, pane.key)
        }
        None => format!("⌘{place} of {counting} → nothing"),
    }
}

/// The step a case asks for, or none for a case that is only about the list.
///
/// `from` is named outright rather than taken from whichever region has focus, so that a case
/// can start from a tab nothing is showing - which is the interesting half, since the whole
/// point of stepping tabs is reaching the ones no region has.
fn read_step(given: &Value) -> Result<Option<(Option<TabId>, TabStep)>, CaseError> {
    let Some(step) = given.get("step") else { return Ok(None) };
    let named = text(step, "direction");
    let direction = TabStep::parse(&named).ok_or_else(|| {
        CaseError::new(format!("`{named}` is not a tab step - write `next` or `previous`"))
    })?;
    let from = step.get("from").and_then(Value::as_str).map(TabId::new);
    Ok(Some((from, direction)))
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
