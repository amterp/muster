//! Composition, written down and read back.
//!
//! The one thing Muster owns that nobody else can answer. A daemon knows its own panes and
//! will hand them back after a reboot; what it has never been told is which of its tabs this
//! window was showing, in what order, at what widths. So that is what gets written, and
//! nothing else - a few hundred bytes of arrangement, no pane trees and no scrollback.
//!
//! Intent rather than observation, which is the line the whole file sits on
//! (`architecture.md`, durability). A region records the tab somebody chose to look at and
//! the width they dragged it to; it does not record what was in that tab, because that is the
//! daemon's to say and it will have moved on. The same rule decides what a daemon entry
//! holds: the endpoint as it was asked for - which does include a socket path when somebody
//! named one, because naming one is the asking - and never the path this run resolved it to.
//!
//! What comes back is therefore a wish that gets checked. A tab the daemon no longer holds is
//! dropped rather than rendered empty, and a window whose regions all fail that check opens
//! the way a first launch does.

use std::collections::BTreeMap;

use crate::composition::presentation::Presentation;
use crate::composition::record::{Composition, Daemon, DaemonId, Endpoint};
use crate::mirror::backend::{PaneId, TabId, WorkspaceId};

/// One region, as much of it as outlives the process.
///
/// No `RegionId`: ids are handed out per run and never reused, so writing one down would name
/// a region that is about to be a different region. Order in the list is the arrangement, and
/// is the only identity a saved region needs.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedRegion {
    pub daemon: DaemonId,
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub weight: f32,
    /// The pane this region's keyboard was on. A wish like the rest: the pane may be gone,
    /// and a region restored without it simply starts wherever the tab's tree begins.
    pub pane: Option<PaneId>,
}

/// A window's arrangement, as it survives a restart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    /// The daemons that were attached, as they were asked for.
    ///
    /// Written so that a window reopens onto the same machines without the config file having
    /// to be the only memory of them - a daemon attached by a CLI or by an agent is one
    /// nothing else wrote down.
    pub daemons: Vec<Daemon>,
    pub regions: Vec<SavedRegion>,
    /// Which region had the keyboard, by its place in the list.
    pub focused: Option<usize>,
    /// The window's own chrome, which needs no checking against a daemon.
    ///
    /// The one thing in this file that comes back exactly as it went in. Everything else is a
    /// wish about a session that may have moved on; nobody else has an opinion about whether
    /// a list was open.
    pub presentation: Presentation,
}

/// What survives a check against what the daemons actually hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restorable {
    pub regions: Vec<SavedRegion>,
    pub focused: Option<usize>,
}

impl Saved {
    /// Takes down what a composition is showing, and what the window is showing of itself.
    pub fn of(composition: &Composition, presentation: Presentation) -> Saved {
        let focused = composition
            .focused_region()
            .and_then(|focused| composition.regions().position(|region| region.id == focused.id));
        Saved {
            presentation,
            daemons: composition.daemons().cloned().collect(),
            regions: composition
                .regions()
                .map(|region| SavedRegion {
                    daemon: region.daemon.clone(),
                    workspace: region.workspace.clone(),
                    tab: region.tab.clone(),
                    weight: region.weight,
                    pane: region.pane.clone(),
                })
                .collect(),
            focused,
        }
    }

    /// The regions worth reopening, given what each daemon turns out to hold.
    ///
    /// A tab that is gone takes its region with it rather than leaving an empty one on
    /// screen: a region showing a tab nobody has is indistinguishable from a daemon that has
    /// not finished describing itself, and one of those is worth waiting for.
    ///
    /// Order is kept, because order is the arrangement. Focus moves to the first survivor
    /// when the region that had it did not make it - a window that opens with the keyboard
    /// nowhere is a window that ignores the first thing typed into it.
    pub fn restorable(&self, holds: impl Fn(&DaemonId, &TabId) -> bool) -> Restorable {
        let mut regions = Vec::new();
        let mut focused = None;
        for (place, region) in self.regions.iter().enumerate() {
            if !holds(&region.daemon, &region.tab) {
                continue;
            }
            if self.focused == Some(place) {
                focused = Some(regions.len());
            }
            regions.push(region.clone());
        }
        if focused.is_none() && !regions.is_empty() {
            focused = Some(0);
        }
        Restorable { regions, focused }
    }
}

/// The version this format is on.
///
/// Written so that a later one can read this and say what it is looking at. A file whose
/// version is not this one is ignored rather than guessed at: the cost of ignoring it is a
/// window that opens the way a first launch does, and the cost of guessing is a window that
/// opens wrong.
const VERSION: i64 = 1;

/// The arrangement as the text that gets written to disk.
///
/// TOML because the config file is TOML and one format is one thing to learn - and because
/// this is a file somebody will open when a window comes back wrong, which rules out anything
/// they would have to decode first.
pub fn to_toml(saved: &Saved) -> String {
    let mut root = toml::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(VERSION));

    let daemons: Vec<toml::Value> = saved.daemons.iter().map(daemon_table).collect();
    if !daemons.is_empty() {
        root.insert("daemon".to_string(), toml::Value::Array(daemons));
    }

    let regions: Vec<toml::Value> = saved.regions.iter().map(region_table).collect();
    if !regions.is_empty() {
        root.insert("region".to_string(), toml::Value::Array(regions));
    }
    if let Some(focused) = saved.focused.and_then(|place| i64::try_from(place).ok()) {
        root.insert("focused".to_string(), toml::Value::Integer(focused));
    }

    // Written even when it matches the default, unlike the keys above that are absent when
    // they have nothing to say. This is a file somebody opens to find out what a window is
    // remembering about them, and a setting that only appears once you have changed it is one
    // they have to already know about to look for.
    let mut window = toml::Table::new();
    window.insert("sidebar".to_string(), toml::Value::Boolean(saved.presentation.sidebar));
    root.insert("window".to_string(), toml::Value::Table(window));

    toml::to_string_pretty(&toml::Value::Table(root))
        .unwrap_or_else(|error| panic!("a composition should always render as TOML: {error}"))
}

fn daemon_table(daemon: &Daemon) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("id".to_string(), toml::Value::String(daemon.id.to_string()));
    match &daemon.endpoint {
        Endpoint::Local { socket_path } => {
            if let Some(path) = socket_path {
                table.insert("socket".to_string(), toml::Value::String(path.clone()));
            }
        }
        Endpoint::Ssh { host, options, socket_path } => {
            table.insert("ssh".to_string(), toml::Value::String(host.clone()));
            if !options.is_empty() {
                table.insert(
                    "ssh_options".to_string(),
                    toml::Value::Array(
                        options.iter().map(|o| toml::Value::String(o.clone())).collect(),
                    ),
                );
            }
            if let Some(path) = socket_path {
                table.insert("socket".to_string(), toml::Value::String(path.clone()));
            }
        }
    }
    toml::Value::Table(table)
}

fn region_table(region: &SavedRegion) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("daemon".to_string(), toml::Value::String(region.daemon.to_string()));
    table.insert("workspace".to_string(), toml::Value::String(region.workspace.to_string()));
    table.insert("tab".to_string(), toml::Value::String(region.tab.to_string()));
    table.insert("weight".to_string(), toml::Value::Float(f64::from(region.weight)));
    if let Some(pane) = &region.pane {
        table.insert("pane".to_string(), toml::Value::String(pane.to_string()));
    }
    toml::Value::Table(table)
}

/// Reads an arrangement back, or says why it will not.
///
/// Every refusal here ends the same way - the window opens as a first launch would - so the
/// message is for whoever is reading a log wondering where their layout went, not for a
/// caller with a second thing to try.
pub fn from_toml(text: &str) -> Result<Saved, String> {
    let root: toml::Table = toml::from_str(text)
        .map_err(|error| format!("the saved arrangement is not TOML: {error}"))?;

    match root.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "the saved arrangement is version {other} and this Muster writes version \
                 {VERSION}. It will open as a first launch does rather than guess at a format \
                 it does not know."
            ));
        }
        None => return Err("the saved arrangement does not say what version it is".to_string()),
    }

    let daemons = root
        .get("daemon")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(read_daemon).collect())
        .unwrap_or_default();
    let regions = root
        .get("region")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(read_region).collect())
        .unwrap_or_default();
    let focused = root
        .get("focused")
        .and_then(toml::Value::as_integer)
        .and_then(|place| usize::try_from(place).ok());

    // Absent means the default, which is what a file written before this key existed looks
    // like. Worth reading that way rather than refusing the file: the version above is for a
    // format that moved, and a key that merely arrived has not moved anything.
    let presentation = Presentation {
        sidebar: root
            .get("window")
            .and_then(toml::Value::as_table)
            .and_then(|window| window.get("sidebar"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(Presentation::default().sidebar),
    };

    Ok(Saved { daemons, regions, focused, presentation })
}

/// An entry that will not read is skipped rather than failing the file, on the same terms as
/// a snapshot: one unreadable region costs that region, and refusing the whole file over it
/// costs the arrangement.
fn read_daemon(value: &toml::Value) -> Option<Daemon> {
    let table = value.as_table()?;
    let id = DaemonId::new(text(table, "id")?);
    let socket_path = text(table, "socket");
    let endpoint = match text(table, "ssh") {
        Some(host) => Endpoint::Ssh {
            host,
            options: table
                .get("ssh_options")
                .and_then(toml::Value::as_array)
                .map(|values| {
                    values.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                })
                .unwrap_or_default(),
            socket_path,
        },
        None => Endpoint::Local { socket_path },
    };
    Some(Daemon { id, endpoint })
}

fn read_region(value: &toml::Value) -> Option<SavedRegion> {
    let table = value.as_table()?;
    Some(SavedRegion {
        daemon: DaemonId::new(text(table, "daemon")?),
        workspace: WorkspaceId::new(text(table, "workspace")?),
        tab: TabId::new(text(table, "tab")?),
        // A region with no readable weight takes an equal share rather than none: zero would
        // be a region the window renders at no width, which nobody can see or grab.
        // Through serde rather than a cast: the same narrowing, without a lint about it.
        weight: table
            .get("weight")
            .cloned()
            .and_then(|weight| weight.try_into::<f32>().ok())
            .filter(|weight: &f32| weight.is_finite() && *weight > 0.0)
            .unwrap_or(1.0),
        pane: text(table, "pane").map(PaneId::new),
    })
}

fn text(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(str::to_string)
}

/// The daemons a saved arrangement names, by id.
///
/// Handed back as a map because that is how a caller uses them: attaching the ones the config
/// file did not already name, without attaching any of them twice.
pub fn daemons_by_id(saved: &Saved) -> BTreeMap<DaemonId, Daemon> {
    saved.daemons.iter().map(|daemon| (daemon.id.clone(), daemon.clone())).collect()
}
