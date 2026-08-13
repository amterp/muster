//! The only state Muster owns, and the one question the input path asks it: which pane does
//! this keyboard feed. Cases and their reasoning live in corpus/conformance/composition.json.
//!
//! The window arrangement, not the input method's composition - `composition_arbiter.rs` is
//! that one (`docs/glossary.md`).

mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, Region, RegionId};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use serde_json::{Value, json};
use support::backend::{read_snapshot, text};

#[test]
fn composition_conformance() {
    let corpus = Conformance::load("composition.json");

    let ran = corpus.run(|given| {
        let worlds = read_worlds(given);
        let mut composition = Composition::new();

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            act(&mut composition, step, &worlds)?;
        }

        // Every field, every case. What a region shows and which pane the keyboard feeds
        // move for reasons a case is usually not about - a daemon detaching, a tab closing
        // two levels up - so a case asserting only its own subject would miss the one that
        // also moved the keyboard.
        Ok(fields([
            ("daemons", Some(json!(describe_daemons(&composition)))),
            ("regions", Some(json!(describe_regions(&composition)))),
            (
                "focusedRegion",
                composition.focused_region().map(|region| json!(region.id.to_string())),
            ),
            ("focusedPane", composition.focused_pane().map(|pane| json!(pane.as_str()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// Runs one step against the composition.
///
/// Steps rather than a single call, because nothing interesting here happens in one: the
/// bugs are in what a reconcile does to a focus somebody set three steps ago.
fn act(
    composition: &mut Composition,
    step: &Value,
    worlds: &BTreeMap<String, Mirror>,
) -> Result<(), CaseError> {
    match text(step, "do").as_str() {
        "attachDaemon" => composition.attach_daemon(Daemon {
            id: daemon(step),
            endpoint: Endpoint::Local { socket_path: text(step, "socketPath") },
        }),
        "detachDaemon" => composition.detach_daemon(&daemon(step)),
        "openRegion" => {
            composition.open_region(
                &daemon(step),
                WorkspaceId::new(text(step, "workspace")),
                TabId::new(text(step, "tab")),
            );
        }
        "closeRegion" => composition.close_region(region(step)?),
        "focusRegion" => composition.focus_region(region(step)?),
        "focusPane" => composition.focus_pane(region(step)?, PaneId::new(text(step, "pane"))),
        "reconcile" => {
            let name = text(step, "world");
            let world = worlds.get(&name).ok_or_else(|| {
                CaseError::new(format!(
                    "the case reconciles against a world it never named: {name}"
                ))
            })?;
            composition.reconcile(&daemon(step), world);
        }
        // Loudly, because a step this driver cannot run would otherwise pass by doing
        // nothing at all.
        other => {
            return Err(CaseError::new(format!(
                "the case names a step the driver does not know: {other:?}"
            )));
        }
    }
    Ok(())
}

fn daemon(step: &Value) -> DaemonId {
    DaemonId::new(text(step, "daemon"))
}

/// A region as a case names it: `r0`, the way it renders.
///
/// Read rather than counted, so that a case says which region it means instead of the
/// reader tracking how many have been opened.
fn region(step: &Value) -> Result<RegionId, CaseError> {
    let named = text(step, "region");
    named.strip_prefix('r').and_then(|number| number.parse().ok()).map(RegionId::new).ok_or_else(
        || CaseError::new(format!("a region is named like r0, and this case says {named:?}")),
    )
}

/// The daemons a case describes, by name, so a step can say which one it is reconciling
/// against.
///
/// Whole snapshots rather than a shorthand of their own: this is the same spelling every
/// other corpus uses for a session, read by the same reader, so a case here and a case in
/// mirror.json cannot come to mean different things.
fn read_worlds(given: &Value) -> BTreeMap<String, Mirror> {
    let mut worlds = BTreeMap::new();
    let described = given.get("worlds").and_then(Value::as_object);
    for (name, snapshot) in described.into_iter().flatten() {
        let mut mirror = Mirror::new();
        mirror.bootstrap(read_snapshot(snapshot));
        worlds.insert(name.clone(), mirror);
    }
    worlds
}

fn describe_daemons(composition: &Composition) -> Vec<String> {
    composition
        .daemons()
        .map(|daemon| match &daemon.endpoint {
            Endpoint::Local { socket_path } => format!("{} local={socket_path}", daemon.id),
        })
        .collect()
}

/// Regions render as readable lines rather than as nested objects.
///
/// `r0 daemon=local workspace=w1 tab=w1:t1 pane=w1:p2` says it where an object makes the
/// reader assemble it (docs/testing.md: bytes render readably). A region with no pane omits
/// the field, because "no pane yet" is a different answer from an empty one and the
/// rendering should not blur them.
fn describe_regions(composition: &Composition) -> Vec<String> {
    composition
        .regions()
        .map(|region: &Region| {
            let mut described = format!(
                "{} daemon={} workspace={} tab={}",
                region.id, region.daemon, region.workspace, region.tab
            );
            if let Some(pane) = &region.pane {
                let _ = write!(described, " pane={pane}");
            }
            described
        })
        .collect()
}
