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
    Composition, Daemon, DaemonId, Endpoint, FontSizeChange, FontSizes, MusterTab, PaneKey, Region,
    RegionId, Step, Transport, View, ViewPane,
};
use muster_core::mirror::Mirror;
use muster_core::mirror::backend::{PaneId, TabId};
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
        // How big each pane's text is. The seam holds this beside the composition rather than
        // in it, and so does this driver: a case that never sizes a pane leaves it empty and
        // every pane in its view reports the configured size.
        let mut sizes = FontSizes::default();

        for step in given.get("steps").and_then(Value::as_array).into_iter().flatten() {
            act(&mut composition, step, given, &worlds, &mut current, &mut sizes)?;
        }

        // Every field, every case. What a region shows and which pane the keyboard feeds
        // move for reasons a case is usually not about - a daemon detaching, a tab closing
        // two levels up - so a case asserting only its own subject would miss the one that
        // also moved the keyboard.
        Ok(fields([
            ("daemons", Some(json!(describe_daemons(&composition)))),
            ("regions", Some(json!(describe_regions(&composition)))),
            // Which of the window's tabs is on screen. Absent when it is showing none, which
            // is a window whose only tabs are another window's - and a real state, since it
            // is what a fresh window looks like until its own workspace arrives.
            ("showingTab", composition.showing().map(|tab| json!(tab.as_str()))),
            (
                "focusedRegion",
                composition.focused_region().map(|region| json!(region.id.to_string())),
            ),
            ("focusedPane", composition.focused_pane().map(|pane| json!(pane.as_str()))),
            ("view", Some(json!(describe_view(&composition, given, &worlds, &current, &sizes)))),
            // Beside the view rather than read off it, because they answer different
            // questions and reading one off the other is the bug this pins: the view says
            // how each region's panes are arranged, and this says which panes the window has
            // on screen. They part company exactly where a tree is withheld or a tab is
            // zoomed, which is where `on_screen` was wrong.
            (
                "showing",
                Some(json!(
                    view_of(&composition, given, &worlds, &current, &sizes)
                        .showing()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<String>>()
                )),
            ),
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
    sizes: &mut FontSizes,
) -> Result<(), CaseError> {
    match text(step, "do").as_str() {
        "attachDaemon" => {
            composition.attach_daemon(Daemon { id: daemon(step), endpoint: endpoint(step) });
        }
        "detachDaemon" => composition.detach_daemon(&daemon(step)),
        "openRegion" => {
            composition.open_region(&daemon(step), TabId::new(text(step, "tab")));
        }
        "closeRegion" => composition.close_region(region(step)?),
        // Which of the window's tabs is on screen, which is what ⌘2, `next_tab` and a click
        // on a caption all come down to.
        "showTab" => {
            composition.show(&TabId::new(text(step, "tab")));
        }
        // What a window somebody asked for records about a machine it has just met: these
        // tabs are another window's, so do not open onto them uninvited.
        "claim" => {
            let tabs = step
                .get("tabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(TabId::new)
                .collect();
            composition.claim(&daemon(step), tabs);
        }
        // The one drag Muster settles for itself: no daemon knows the other one exists, so
        // nothing upstream can say how a window divides between them.
        "setBoundary" => composition.set_boundary(region(step)?, ratio(step)),
        // What following a notification does before it moves the keyboard: the pane that
        // asked may be in a tab no region is showing, and surfacing it is the core's job.
        "surface" => {
            composition.surface(&daemon(step), &TabId::new(text(step, "tab")));
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
                view_of(composition, given, worlds, current, sizes).step(direction)
            {
                composition.focus_pane(region, pane);
            }
        }
        // One press of a font-size chord, on the pane the keyboard is on - which is what the
        // seam does with it. Named rather than given a number, because what a chord means is
        // "one more than whatever I have" and that is the half worth pinning.
        "adjustFontSize" => {
            let named = text(step, "change");
            let change = FontSizeChange::parse(&named).ok_or_else(|| {
                CaseError::new(format!(
                    "a font size change is larger, smaller or reset, and this case says \
                     {named:?}"
                ))
            })?;
            let pane = composition
                .focused_region()
                .and_then(|region| Some(PaneKey::new(&region.daemon, region.pane.as_ref()?)))
                .ok_or_else(|| {
                    CaseError::new(
                        "the case sizes text with no pane holding the keyboard, which the seam \
                         refuses rather than acts on"
                            .to_string(),
                    )
                })?;
            sizes.adjust(&pane, change);
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
    sizes: &FontSizes,
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
        |daemon, pane| ViewPane {
            id: pane.clone(),
            control_socket_path: attached
                .contains(&pane.to_string())
                .then(|| format!("/tmp/{daemon}-{pane}.sock")),
            // A case's panes are named the way the backend names them, so the two spellings
            // agree and neither the cases nor this driver has to know the registry exists.
            // What the translation does is `pane-names.json`'s subject.
            backend_pane_id: Some(pane.to_string()),
            font_size_offset: sizes.offset(&PaneKey::new(daemon, pane)),
            // Zero, because no case here is about a bridge that had to be replaced - what a
            // replacement does to a window is `respawn.json`'s subject, and a number in this
            // driver would only ever restate its own input.
            bridge_restarts: 0,
        },
    )
}

fn describe_view(
    composition: &Composition,
    given: &Value,
    worlds: &BTreeMap<String, Mirror>,
    current: &BTreeMap<DaemonId, String>,
    sizes: &FontSizes,
) -> Vec<String> {
    view_of(composition, given, worlds, current, sizes)
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

/// Every region the window holds, across every tab, as readable lines.
///
/// `r0 daemon=local tab=w1:t1 pane=w1:p2` says it where an object makes the reader assemble it
/// (docs/testing.md: bytes render readably). A region with no pane omits the field, because "no
/// pane yet" is a different answer from an empty one and the rendering should not blur them.
///
/// Every tab's, not only the tab on screen: a tab in the background keeps its arrangement, and a
/// case that asserted only what is drawn could not say so. Which of them is on screen is
/// `showingTab`, and what that adds up to is `view`.
fn describe_regions(composition: &Composition) -> Vec<String> {
    composition
        .tabs()
        .flat_map(|tab| tab.regions().map(move |region| (tab, region)))
        .map(|(tab, region): (&MusterTab, &Region)| {
            let mut described = format!("{} daemon={} tab={}", region.id, region.daemon, tab.id);
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
