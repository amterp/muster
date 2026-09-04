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

use crate::composition::presentation::{FontSizes, Frame, Presentation};
use crate::composition::record::{Composition, Daemon, DaemonId, Endpoint, PaneKey};
use crate::diagnostics::log;
use crate::fields;
use crate::mirror::backend::{PaneId, TabId};

/// One machine's half of a tab, as much of it as outlives the process.
///
/// No `RegionId`: ids are handed out per run and never reused, so writing one down would name
/// a region that is about to be a different region. Order in the list is the arrangement, and
/// is the only identity a saved region needs.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedRegion {
    pub daemon: DaemonId,
    pub weight: f32,
    /// The pane this region's keyboard was on. A wish like the rest: the pane may be gone,
    /// and a region restored without it simply starts wherever the tab's tree begins.
    pub pane: Option<PaneId>,
    /// Whether this was the region the keyboard was in while its tab was on screen.
    ///
    /// Per region rather than an index into the tab's list, because the list is filtered on the
    /// way back in - a machine that has gone takes its region with it - and an index into a
    /// filtered list points at whatever moved up. At most one region of a tab carries it.
    pub keyboard: bool,
}

/// One Muster tab, as much of it as outlives the process.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedTab {
    pub id: TabId,
    /// One per machine holding panes in it, in the order they are laid out.
    pub regions: Vec<SavedRegion>,
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
    /// The tabs this window held, in the order they are walked.
    pub tabs: Vec<SavedTab>,
    /// Which of them was on screen.
    pub showing: Option<TabId>,
    /// Tabs a migration folded into another, as pairs of the tab that stayed and the tab that
    /// became a member of it.
    ///
    /// Empty for every file this Muster wrote. A version 3 arrangement is a column per machine
    /// and each column named a tab of its own, so reading one back as a window holding one tab
    /// per column would take away the side-by-side view somebody was using. It becomes one
    /// Muster tab holding all of it instead, which needs those tabs grouped under one name in
    /// the name registry - and the registry is not this file's to write. So the file says what
    /// it implied, and whoever holds the registry acts on it.
    pub grouped: Vec<(TabId, TabId)>,
    /// The window's own chrome, which needs no checking against a daemon.
    ///
    /// The only part of this file no daemon has an opinion about: nobody else knows whether a
    /// list was open. The frame is checked all the same, but against the screens the machine
    /// has rather than against a session (`Frame::fitted`).
    pub presentation: Presentation,

    /// How big each pane's text was, for the panes somebody had sized.
    ///
    /// Beside the chrome rather than inside it, because it is not one answer about the window.
    /// Restored without checking, unlike a region: an entry for a pane that is gone costs a
    /// forgotten row and nothing on screen, where a region for a tab that is gone is a region
    /// rendering nothing. The pruning happens while the window runs, where a daemon that is
    /// actually answering can say which panes it still holds.
    pub font_sizes: FontSizes,
}

/// What survives a check against what the daemons actually hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restorable {
    pub tabs: Vec<SavedTab>,
    pub showing: Option<TabId>,
}

impl Saved {
    /// Takes down what a composition is showing, and what the window is showing of itself.
    pub fn of(
        composition: &Composition,
        presentation: Presentation,
        font_sizes: &FontSizes,
    ) -> Saved {
        Saved {
            presentation,
            font_sizes: font_sizes.clone(),
            daemons: composition.daemons().cloned().collect(),
            showing: composition.showing().cloned(),
            grouped: Vec::new(),
            tabs: composition
                .tabs()
                .map(|tab| {
                    let keyboard = tab.focused_region().map(|region| region.id);
                    SavedTab {
                        id: tab.id.clone(),
                        regions: tab
                            .regions()
                            .map(|region| SavedRegion {
                                daemon: region.daemon.clone(),
                                weight: region.weight,
                                pane: region.pane.clone(),
                                keyboard: keyboard == Some(region.id),
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    /// The tabs worth reopening, given what each daemon turns out to hold.
    ///
    /// A machine that no longer holds its half of a tab loses that region rather than leaving
    /// an empty one on screen: a region showing a tab nobody has is indistinguishable from a
    /// daemon that has not finished describing itself, and one of those is worth waiting for. A
    /// tab that loses every region goes with them, and a tab that loses one of two opens showing
    /// the machine still there, which is what a tab whose devenv is unreachable has to do.
    ///
    /// Order is kept, because order is the arrangement. The tab that was on screen falls back to
    /// the first survivor - a window that opens showing nothing while it holds tabs is a window
    /// that ignores the first thing typed into it.
    pub fn restorable(&self, holds: impl Fn(&DaemonId, &TabId) -> bool) -> Restorable {
        let tabs: Vec<SavedTab> = self
            .tabs
            .iter()
            .map(|tab| SavedTab {
                id: tab.id.clone(),
                regions: tab
                    .regions
                    .iter()
                    .filter(|region| holds(&region.daemon, &tab.id))
                    .cloned()
                    .collect(),
            })
            .filter(|tab| !tab.regions.is_empty())
            .collect();
        let showing = self
            .showing
            .clone()
            .filter(|showing| tabs.iter().any(|tab| &tab.id == showing))
            .or_else(|| tabs.first().map(|tab| tab.id.clone()));
        Restorable { tabs, showing }
    }
}

/// The version this format is on.
///
/// Written so that a later one can read this and say what it is looking at. A file whose
/// version is not this one is ignored rather than guessed at: the cost of ignoring it is a
/// window that opens the way a first launch does, and the cost of guessing is a window that
/// opens wrong.
///
/// **2 because `region.tab` changed meaning.** A version 1 file holds the backend's tab id,
/// which no longer resolves now that Muster mints its own (`crate::names`). Left at 1 the file
/// still parses and every region silently fails its check, so the arrangement vanishes with
/// nothing said; refused by version, the log names what it found and why the window opened
/// fresh. One lost arrangement either way - this is the one that explains itself.
///
/// **3 because a region stopped naming a workspace.** A version 2 file carries one and nothing
/// reads it any more: a workspace is the backend's unit for a whole project, and a tab already
/// says which one it is in (MIP-2). A version 2 file would parse and restore correctly with the
/// key ignored, so this bump buys less than the last one - what it buys is that the file on
/// disk and the format this reads never differ silently, which is the property a version is for.
const VERSION: i64 = 4;

/// The version that wrote a column per machine, which this one still reads.
///
/// Version 3 named a tab on every `[[region]]` and had no notion of a window's tab list, because
/// the window was the columns. Read rather than refused, and read as one Muster tab holding every
/// column, so the first launch after this lands looks like the last launch before it - see
/// [`into_one_tab`].
const COLUMN_PER_MACHINE: i64 = 3;

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

    // Flat, with each row naming its tab, rather than nested under `[[tab]]`. The tabs are the
    // order their rows first appear in, so nothing has to be stated twice - and a person opening
    // this file to find out why a window came back wrong reads a list of rows rather than a
    // nesting TOML spells awkwardly.
    let regions: Vec<toml::Value> = saved
        .tabs
        .iter()
        .flat_map(|tab| tab.regions.iter().map(move |region| region_table(&tab.id, region)))
        .collect();
    if !regions.is_empty() {
        root.insert("region".to_string(), toml::Value::Array(regions));
    }

    // Only the panes somebody has sized, unlike the `[window]` keys below. Those are a fixed
    // set worth stating at their default so a reader learns they exist; this is a list of
    // exceptions, and a row per pane saying "the configured size" would be a table that grows
    // with the window and says nothing.
    let panes: Vec<toml::Value> = saved.font_sizes.entries().map(pane_table).collect();
    if !panes.is_empty() {
        root.insert("pane".to_string(), toml::Value::Array(panes));
    }
    if let Some(showing) = &saved.showing {
        root.insert("showing".to_string(), toml::Value::String(showing.to_string()));
    }

    // Written even when it matches the default, unlike the keys above that are absent when
    // they have nothing to say. This is a file somebody opens to find out what a window is
    // remembering about them, and a setting that only appears once you have changed it is one
    // they have to already know about to look for.
    let mut window = toml::Table::new();
    window.insert("sidebar".to_string(), toml::Value::Boolean(saved.presentation.sidebar));
    window.insert("full_screen".to_string(), toml::Value::Boolean(saved.presentation.full_screen));
    // The four numbers go flat beside the two above rather than into a table of their own. They
    // are the same kind of value - what the window looked like, not what session it was
    // showing - and a level of nesting for four scalars makes this file slower to read for
    // nothing.
    //
    // Absent rather than zeroed when there is no frame, unlike the keys above. A window that has
    // never settled has no rectangle to state, and `x = 0` would be a claim about a corner of a
    // display rather than the absence of one.
    if let Some(frame) = saved.presentation.frame {
        window.insert("x".to_string(), toml::Value::Float(frame.x));
        window.insert("y".to_string(), toml::Value::Float(frame.y));
        window.insert("width".to_string(), toml::Value::Float(frame.width));
        window.insert("height".to_string(), toml::Value::Float(frame.height));
    }
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

/// One pane somebody sized, named the way a region names its own.
///
/// Both halves of the key, although a pane's Muster name is already unique across every
/// attached machine. It reads beside `[[region]]`, which spells its pane the same way, and a
/// row that says which machine it belongs to is one somebody can act on when they find it.
fn pane_table((pane, offset): (&PaneKey, i32)) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("daemon".to_string(), toml::Value::String(pane.daemon.to_string()));
    table.insert("pane".to_string(), toml::Value::String(pane.pane.to_string()));
    table.insert("font_size_offset".to_string(), toml::Value::Integer(i64::from(offset)));
    toml::Value::Table(table)
}

fn region_table(tab: &TabId, region: &SavedRegion) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("tab".to_string(), toml::Value::String(tab.to_string()));
    table.insert("daemon".to_string(), toml::Value::String(region.daemon.to_string()));
    table.insert("weight".to_string(), toml::Value::Float(f64::from(region.weight)));
    if let Some(pane) = &region.pane {
        table.insert("pane".to_string(), toml::Value::String(pane.to_string()));
    }
    // Only on the one region of a tab that had it, so a reader can find the keyboard by looking
    // for the word rather than by counting rows.
    if region.keyboard {
        table.insert("keyboard".to_string(), toml::Value::Boolean(true));
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

    let version = match root.get("version").and_then(toml::Value::as_integer) {
        Some(version @ (VERSION | COLUMN_PER_MACHINE)) => version,
        Some(other) => {
            return Err(format!(
                "the saved arrangement is version {other} and this Muster writes version \
                 {VERSION}. It will open as a first launch does rather than guess at a format \
                 it does not know."
            ));
        }
        None => return Err("the saved arrangement does not say what version it is".to_string()),
    };

    let daemons = root
        .get("daemon")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(read_daemon).collect())
        .unwrap_or_default();
    let rows: Vec<(TabId, SavedRegion)> = root
        .get("region")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(read_region).collect())
        .unwrap_or_default();
    let (mut tabs, grouped) = match version {
        COLUMN_PER_MACHINE => into_one_tab(rows, &root),
        _ => (into_tabs(rows), Vec::new()),
    };
    let showing = root
        .get("showing")
        .and_then(toml::Value::as_str)
        .map(TabId::new)
        .filter(|showing| tabs.iter().any(|tab| &tab.id == showing))
        .or_else(|| tabs.first().map(|tab| tab.id.clone()));
    // A tab with no region on any machine is not a tab, and a hand-edited file can say one.
    tabs.retain(|tab| !tab.regions.is_empty());

    // Absent means the default, which is what a file written before this key existed looks
    // like. Worth reading that way rather than refusing the file: the version above is for a
    // format that moved, and a key that merely arrived has not moved anything.
    let window = root.get("window").and_then(toml::Value::as_table);
    let presentation = Presentation::default()
        .with_sidebar(
            window
                .and_then(|window| window.get("sidebar"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(Presentation::default().sidebar),
        )
        .with_frame(
            window.and_then(read_frame),
            window
                .and_then(|window| window.get("full_screen"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(Presentation::default().full_screen),
        );

    report_window_wide_text_size(window);
    let font_sizes = root
        .get("pane")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(read_pane_font_size).collect())
        .unwrap_or_default();

    Ok(Saved { daemons, tabs, showing, grouped, presentation, font_sizes })
}

/// Says so when a file remembers a text size for the whole window.
///
/// Left over from when `cmd+=` sized every pane at once. There is nowhere to put it now - the
/// file names the panes it remembers and this named none of them - so it is dropped, and the
/// one thing worth doing about it is saying so where somebody wondering why their text is
/// small again will find it.
///
/// Not a problem raised into the roster: nothing is broken and nothing needs a human edit.
/// One record, once, on the launch that reads the old file - the next save writes the new
/// shape and this never fires again.
fn report_window_wide_text_size(window: Option<&toml::Table>) {
    let offset = window
        .and_then(|window| window.get("font_size_offset"))
        .and_then(toml::Value::as_integer)
        .unwrap_or_default();
    if offset == 0 {
        return;
    }
    log::info(
        "state.font_size.window_wide",
        fields! {
            "offset" => offset.to_string(),
            "impact" => "text is sized per pane now, so this window-wide size was dropped and \
                         every pane opens at the size the config file names",
            "check" => "size the panes you want bigger again and it is remembered per pane; \
                        `[font] size` in the config file is what moves all of them at once",
        },
    );
}

/// An entry that will not read is skipped rather than failing the file, on the same terms as a
/// region: one unreadable row costs one pane's text size, and refusing the whole file over it
/// costs the arrangement.
fn read_pane_font_size(value: &toml::Value) -> Option<(PaneKey, i32)> {
    let table = value.as_table()?;
    let key =
        PaneKey::new(&DaemonId::new(text(table, "daemon")?), &PaneId::new(text(table, "pane")?));
    let offset = table.get("font_size_offset")?.as_integer().and_then(|o| i32::try_from(o).ok())?;
    Some((key, offset))
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

fn read_region(value: &toml::Value) -> Option<(TabId, SavedRegion)> {
    let table = value.as_table()?;
    let tab = TabId::new(text(table, "tab")?);
    let region = SavedRegion {
        daemon: DaemonId::new(text(table, "daemon")?),
        keyboard: table.get("keyboard").and_then(toml::Value::as_bool).unwrap_or_default(),
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
    };
    Some((tab, region))
}

/// The rows gathered into tabs, in the order each tab's first row appears.
fn into_tabs(rows: Vec<(TabId, SavedRegion)>) -> Vec<SavedTab> {
    let mut tabs: Vec<SavedTab> = Vec::new();
    for (tab, region) in rows {
        match tabs.iter_mut().find(|held| held.id == tab) {
            Some(held) => held.regions.push(region),
            None => tabs.push(SavedTab { id: tab, regions: vec![region] }),
        }
    }
    tabs
}

/// A column-per-machine arrangement, read as the one Muster tab holding all of it.
///
/// The decision this implements is that an upgrade does not take away what somebody is looking
/// at: a window showing a laptop column beside a devenv column comes back showing both, side by
/// side, in one tab. Splitting that into a tab per machine is something to do afterwards and on
/// purpose, once the new model is on screen to do it in.
///
/// The tab that stays is the first column's, and the rest become members of it - which is the
/// grouping reported in `Saved::grouped` for whoever holds the name registry to apply. Two
/// columns on one machine collapse into one region, which is what a file written by a window
/// that had drawn a pane twice looks like (kan a_2Ht74jTXV).
fn into_one_tab(
    rows: Vec<(TabId, SavedRegion)>,
    root: &toml::Table,
) -> (Vec<SavedTab>, Vec<(TabId, TabId)>) {
    let focused = root
        .get("focused")
        .and_then(toml::Value::as_integer)
        .and_then(|place| usize::try_from(place).ok())
        .unwrap_or_default();
    let Some(into) = rows.first().map(|(tab, _)| tab.clone()) else {
        return (Vec::new(), Vec::new());
    };

    let mut regions: Vec<SavedRegion> = Vec::new();
    let mut grouped: Vec<(TabId, TabId)> = Vec::new();
    for (place, (tab, mut region)) in rows.into_iter().enumerate() {
        if tab != into && !grouped.iter().any(|(_, absorbed)| absorbed == &tab) {
            grouped.push((into.clone(), tab));
        }
        if regions.iter().any(|held| held.daemon == region.daemon) {
            continue;
        }
        region.keyboard = place == focused;
        regions.push(region);
    }
    if !regions.iter().any(|region| region.keyboard)
        && let Some(first) = regions.first_mut()
    {
        first.keyboard = true;
    }
    (vec![SavedTab { id: into, regions }], grouped)
}

fn text(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(str::to_string)
}

/// The rectangle a `[window]` table names, if it names a whole one.
///
/// All four or none, and a size has to be a size. A partial set is the same answer as no set at
/// all - a window has no meaning at three quarters of a rectangle, and a height of zero is a
/// window nobody can see or grab. Both are hand-edits rather than anything Muster writes, so the
/// answer is to open where a first launch would rather than to refuse the file and lose the
/// arrangement with it.
fn read_frame(window: &toml::Table) -> Option<Frame> {
    let number = |key: &str| {
        window
            .get(key)
            .and_then(|value| match value {
                toml::Value::Float(number) => Some(*number),
                // Integers too, because a person typing a rectangle by hand writes `x = 120`.
                // Through i32 rather than straight to f64: a coordinate past two billion points
                // is not a screen anybody has, so losing precision on it would be dressing up a
                // typo as a rectangle.
                toml::Value::Integer(number) => i32::try_from(*number).ok().map(f64::from),
                _ => None,
            })
            .filter(|number: &f64| number.is_finite())
    };
    let frame = Frame {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    };
    (frame.width > 0.0 && frame.height > 0.0).then_some(frame)
}

/// The daemons a saved arrangement names, by id.
///
/// Handed back as a map because that is how a caller uses them: attaching the ones the config
/// file did not already name, without attaching any of them twice.
pub fn daemons_by_id(saved: &Saved) -> BTreeMap<DaemonId, Daemon> {
    saved.daemons.iter().map(|daemon| (daemon.id.clone(), daemon.clone())).collect()
}
