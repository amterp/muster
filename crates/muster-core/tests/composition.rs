//! The only state Muster owns, and the one question the input path asks it: which pane does
//! this keyboard feed. Cases and their reasoning live in corpus/conformance/composition.json.
//!
//! The window arrangement, not the input method's composition - `composition_arbiter.rs` is
//! that one (`docs/glossary.md`).

mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::{
    Composition, Daemon, DaemonId, Endpoint, Region, RegionId, Step, Transport, View,
};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use serde_json::{Value, json};
use support::backend::{describe_daemon, optional, ratio, read_snapshot, text};

#[test]
fn composition_conformance() {
    let corpus = Conformance::load("composition.json");

    let ran = corpus.run(|given| {
        let worlds = read_worlds(given);
        let mut composition = Composition::new();
        // Which world each daemon has most recently said is true. A view is computed
        // against whatever each daemon last published, and a daemon that has said nothing
        // yet is a real state - one attached whose subscription has not bootstrapped.
        let mut current: BTreeMap<DaemonId, String> = BTreeMap::new();

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            act(&mut composition, step, given, &worlds, &mut current)?;
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
            ("view", Some(json!(describe_view(&composition, given, &worlds, &current)))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn every_direction_is_in_the_corpus() {
    // A direction added to the enum and not here is a key that does something nothing
    // decided. Both halves matter and fail differently: an unspelled ordinal step reaches
    // every pane in some order nobody chose, and an unspelled arrow lands wherever the
    // geometry happens to put it.
    let corpus = Conformance::load("composition.json");
    let stepped: Vec<String> = corpus
        .cases
        .iter()
        .filter_map(|case| case.given.get("steps").and_then(Value::as_array))
        .flatten()
        .filter(|step| text(step, "do") == "step")
        .map(|step| text(step, "direction"))
        .collect();

    for direction in Step::ALL {
        assert!(
            stepped.iter().any(|named| named == direction.as_str()),
            "no corpus case steps `{}`, so nothing pins where it lands",
            direction.as_str()
        );
    }
}

/// Runs one step against the composition.
///
/// Steps rather than a single call, because nothing interesting here happens in one: the
/// bugs are in what a reconcile does to a focus somebody set three steps ago.
fn act(
    composition: &mut Composition,
    step: &Value,
    given: &Value,
    worlds: &BTreeMap<String, Mirror>,
    current: &mut BTreeMap<DaemonId, String>,
) -> Result<(), CaseError> {
    match text(step, "do").as_str() {
        "attachDaemon" => {
            composition.attach_daemon(Daemon { id: daemon(step), endpoint: endpoint(step) });
        }
        "detachDaemon" => composition.detach_daemon(&daemon(step)),
        "openRegion" => {
            composition.open_region(
                &daemon(step),
                WorkspaceId::new(text(step, "workspace")),
                TabId::new(text(step, "tab")),
            );
        }
        "closeRegion" => composition.close_region(region(step)?),
        // The one drag Muster settles for itself: no daemon knows the other one exists, so
        // nothing upstream can say how a window divides between them.
        "setBoundary" => composition.set_boundary(region(step)?, ratio(step)),
        // What following a notification does before it moves the keyboard: the pane that
        // asked may be in a tab no region is showing, and surfacing it is the core's job.
        "surface" => {
            composition.surface(
                &daemon(step),
                WorkspaceId::new(text(step, "workspace")),
                TabId::new(text(step, "tab")),
            );
        }
        "focusRegion" => composition.focus_region(region(step)?),
        "focusPane" => composition.focus_pane(region(step)?, PaneId::new(text(step, "pane"))),
        // The seam's own two lines: ask the view where a step lands, then move the keyboard
        // there. Written out here rather than wrapped in the core, because what the core
        // owns is the answer and the applying is one call.
        "step" => {
            let named = text(step, "direction");
            let direction = Step::parse(&named).ok_or_else(|| {
                CaseError::new(format!(
                    "a step goes next or previous, and this case says {named:?}"
                ))
            })?;
            if let Some((region, pane)) =
                view_of(composition, given, worlds, current).step(direction)
            {
                composition.focus_pane(region, pane);
            }
        }
        "reconcile" => {
            let name = text(step, "world");
            let world = worlds.get(&name).ok_or_else(|| {
                CaseError::new(format!(
                    "the case reconciles against a world it never named: {name}"
                ))
            })?;
            composition.reconcile(&daemon(step), world);
            current.insert(daemon(step), name);
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

/// How a case says a daemon is reached.
///
/// A `host` makes it remote, exactly as it does in the config file this mirrors. Cases that
/// name neither get a local daemon nobody has told where to look, which is what a config
/// naming one daemon and no socket produces.
fn endpoint(step: &Value) -> Endpoint {
    let socket_path = optional(step, "socketPath");
    match optional(step, "host") {
        Some(host) => Endpoint::Ssh {
            host,
            options: step
                .get("sshOptions")
                .and_then(Value::as_array)
                .map(|options| {
                    options.iter().filter_map(Value::as_str).map(str::to_string).collect()
                })
                .unwrap_or_default(),
            socket_path,
        },
        None => Endpoint::Local { socket_path },
    }
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

/// The view each case's composition adds up to, one line per region.
///
/// Asserted alongside the records rather than in a corpus of its own, because the view is a
/// projection of exactly these records and nothing else: a case that moved a region and
/// asserted only the record would leave what a window shows unstated.
///
/// `attached` in the given names the panes a channel is open for, spelled by pane alone -
/// good enough here, where no case needs two daemons to differ in which of their panes are
/// attached.
fn view_of(
    composition: &Composition,
    given: &Value,
    worlds: &BTreeMap<String, Mirror>,
    current: &BTreeMap<DaemonId, String>,
) -> View {
    let attached: Vec<String> = given
        .get("attached")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pane| pane.as_str())
        .map(str::to_string)
        .collect();

    View::of(
        composition,
        |daemon| worlds.get(current.get(daemon)?),
        |daemon, pane| {
            attached.contains(&pane.to_string()).then(|| format!("/tmp/{daemon}-{pane}.sock"))
        },
        // How a daemon is reached is the runtime's answer, and a case has no runtime. What a
        // case does say is how a daemon was asked for, so this reads the endpoint back: a
        // region on an ssh daemon carries a transport, and one on a local daemon does not.
        |daemon| match composition.daemon(daemon).map(|held| &held.endpoint) {
            Some(Endpoint::Ssh { host, .. }) => {
                Some(Transport { host: host.clone(), control_path: format!("/tmp/{daemon}.ctl") })
            }
            _ => None,
        },
        // The mirror image, and read back the same way: a local daemon says where its frames
        // come from, and a remote one does not, because that bridge asks the far machine.
        |daemon| match composition.daemon(daemon).map(|held| &held.endpoint) {
            Some(Endpoint::Ssh { .. }) => None,
            _ => Some(format!("/tmp/{daemon}.sock")),
        },
        // A case's panes are named the way the backend names them, so the two spellings agree
        // and neither the cases nor this driver has to know the registry exists. What the
        // translation does is `pane-names.json`'s subject.
        |_, pane| Some(pane.to_string()),
    )
}

fn describe_view(
    composition: &Composition,
    given: &Value,
    worlds: &BTreeMap<String, Mirror>,
    current: &BTreeMap<DaemonId, String>,
) -> Vec<String> {
    view_of(composition, given, worlds, current)
        .regions
        .iter()
        .map(|region| {
            let mut described = format!("{} tab={}", region.id, region.tab);
            match &region.root {
                // Distinct from a region with no panes, and the corpus says so: a shell
                // told this leaves its surfaces alone rather than tearing them down.
                None => described.push_str(" (no tree)"),
                Some(root) => {
                    let _ = write!(described, " {root}");
                    if region.zoomed {
                        described.push_str(" zoomed");
                    }
                }
            }
            described
        })
        .collect()
}

fn describe_daemons(composition: &Composition) -> Vec<String> {
    composition.daemons().map(describe_daemon).collect()
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
            // Only when it has moved, so that the great majority of cases - which are not
            // about widths - stay readable. Rounded, because a share is arrived at by
            // division and a case should not pin the last bit of a float.
            if (region.weight - 1.0).abs() > 0.0005 {
                let _ = write!(described, " weight={:.3}", region.weight);
            }
            if let Some(pane) = &region.pane {
                let _ = write!(described, " pane={pane}");
            }
            described
        })
        .collect()
}
