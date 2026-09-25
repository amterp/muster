//! Which window holds each tab.
//!
//! **Every tab belongs to exactly one window** (kan a_2Mhi0EZlv). A window lists the tabs it
//! holds and no others, so its sidebar, its numbered chords and its notifications are about its
//! own work. No tab is ever in two windows: herdr allows one client per terminal, so a tab
//! listed in two windows is a tab whose terminals one of them takes from the other at the first
//! click. Measured that way on 0.7.0 and again on 0.8.1, where a window holding nothing drew the
//! tab another window had asked for.
//!
//! No daemon can hold this, because a daemon does not know which windows exist. So it is
//! Muster's own record, shared by every window the way pane names are (`crate::shared`), and
//! the file is the only place the answer lives: a window reads it, changes it and writes it
//! inside one hold, and another window hears that it moved.
//!
//! A window here is its arrangement record rather than its process (MIP-2): `window-2` holds
//! its tabs across a quit, which is what lets `muster window reopen` come back onto them and
//! stops two windows reopening onto one tab. The pid and socket are only while it is open.
//!
//! Pure: no clock, no socket, no file. Whether a window is open is asked of the caller, which
//! dials its socket, and so is the time.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::DaemonId;
use crate::mirror::backend::{TabId, id_type};

id_type!(
    WindowName,
    "Which window a tab belongs to: its arrangement record's name, such as `window-2`."
);

/// One window the record knows of, open or closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldWindow {
    pub name: WindowName,
    /// Where its arrangement is written. Empty for a window told to remember nothing, which
    /// cannot be reopened and so holds its tabs only while it is open.
    pub arrangement: String,
    /// The command socket it answers on. Empty once it has closed.
    pub socket: String,
    /// Its process, while it is open, and zero once it has closed. What `muster window` prints
    /// and what `muster tab move --window` accepts, so a person can copy one into the other.
    pub pid: u32,
    /// When it last came to the front, in milliseconds since the epoch. Decides which window a
    /// tab nobody holds joins.
    pub focused: i64,
    /// The machines it follows. Every window follows this one's daemon, but only a window
    /// attached to a devenv follows that one, and a window that cannot see a tab cannot take it.
    pub daemons: BTreeSet<DaemonId>,
}

/// A window about to ask a machine for a tab it has not been told the name of yet.
///
/// Written before the request and cleared once the answer names the tab. Between the two the
/// daemon may already have described that tab to every window, and without this, the rule for
/// a tab nobody holds would hand it to whichever window came to the front last - which is the
/// window that did not ask for it, as often as not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expecting {
    pub window: WindowName,
    pub daemon: DaemonId,
    /// When it was written, in milliseconds since the epoch.
    pub since: i64,
}

/// Who takes a tab nobody holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taker {
    /// The window that came to the front most recently, of the ones that are open.
    Window(WindowName),
    /// Nobody yet: a window is waiting on an answer from this machine, and the tab may be its.
    /// Decided again once that window writes what it got.
    Waiting(WindowName),
    /// No window is open. The next one to open takes it.
    Nobody,
}

/// The whole record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holders {
    windows: BTreeMap<WindowName, HeldWindow>,
    tabs: BTreeMap<TabId, WindowName>,
    expecting: Vec<Expecting>,
}

impl Holders {
    pub fn new() -> Holders {
        Holders::default()
    }

    /// The window holding a tab, if one does.
    pub fn holder(&self, tab: &TabId) -> Option<&WindowName> {
        self.tabs.get(tab)
    }

    pub fn window(&self, name: &WindowName) -> Option<&HeldWindow> {
        self.windows.get(name)
    }

    pub fn windows(&self) -> impl Iterator<Item = &HeldWindow> {
        self.windows.values()
    }

    /// Every tab a window holds, in name order - which is the order they were made.
    pub fn held_by<'a>(&'a self, window: &'a WindowName) -> impl Iterator<Item = &'a TabId> + 'a {
        self.tabs.iter().filter(move |(_, holder)| *holder == window).map(|(tab, _)| tab)
    }

    /// Gives a tab to a window, taking it from whichever window held it.
    ///
    /// A window that has not said it exists is recorded all the same, closed. A tab moved to a
    /// closed window is what `muster tab move --window window-2` does, and it comes back when
    /// that window is reopened.
    pub fn take(&mut self, tab: TabId, window: &WindowName) {
        self.windows.entry(window.clone()).or_insert_with(|| HeldWindow {
            name: window.clone(),
            arrangement: String::new(),
            socket: String::new(),
            pid: 0,
            focused: 0,
            daemons: BTreeSet::new(),
        });
        self.tabs.insert(tab, window.clone());
    }

    /// Says a window is open, and where to reach it.
    ///
    /// Keeps what the record already says about which tabs it holds, because that is the point
    /// of naming a window after its arrangement: it comes back to them.
    pub fn opened(&mut self, window: HeldWindow) {
        self.windows.insert(window.name.clone(), window);
    }

    /// Says a window has closed. It keeps its tabs.
    pub fn closed(&mut self, name: &WindowName) {
        if let Some(window) = self.windows.get_mut(name) {
            window.socket.clear();
            window.pid = 0;
        }
        self.expecting.retain(|expecting| &expecting.window != name);
    }

    /// Says which machines a window follows now, which changes when it attaches one.
    pub fn follows(&mut self, name: &WindowName, daemons: BTreeSet<DaemonId>) {
        if let Some(window) = self.windows.get_mut(name) {
            window.daemons = daemons;
        }
    }

    pub fn focused(&mut self, name: &WindowName, at: i64) {
        if let Some(window) = self.windows.get_mut(name) {
            window.focused = at;
        }
    }

    /// Says this window is about to ask this machine for a tab.
    pub fn expect(&mut self, window: &WindowName, daemon: &DaemonId, at: i64) {
        self.expected(window, daemon);
        self.expecting.push(Expecting {
            window: window.clone(),
            daemon: daemon.clone(),
            since: at,
        });
    }

    /// Says the answer arrived, whether or not it named a tab.
    pub fn expected(&mut self, window: &WindowName, daemon: &DaemonId) {
        self.expecting
            .retain(|expecting| &expecting.window != window || &expecting.daemon != daemon);
    }

    /// Forgets the windows that can never come back, and lets their tabs go.
    ///
    /// A window whose arrangement is gone - pruned, deleted, or never written because it was
    /// told to remember nothing - cannot be reopened, so a tab it held would otherwise be held
    /// by nobody who could ever show it. Let go, it joins an open window under the rule for a
    /// tab nobody holds.
    pub fn forget(&mut self, gone: impl Fn(&HeldWindow) -> bool) {
        let forgotten: Vec<WindowName> =
            self.windows.values().filter(|window| gone(window)).map(|w| w.name.clone()).collect();
        for name in &forgotten {
            self.windows.remove(name);
            self.expecting.retain(|expecting| &expecting.window != name);
        }
        self.tabs.retain(|_, holder| !forgotten.contains(holder));
    }

    /// Drops the tabs that no longer exist anywhere.
    ///
    /// `known` is whether a tab's name still resolves. Tab names are never reused, so a row for
    /// one that resolves nowhere is a row nothing will ever read again.
    pub fn prune(&mut self, known: impl Fn(&TabId) -> bool) {
        self.tabs.retain(|tab, _| known(tab));
    }

    /// Which window takes a tab nobody holds on this machine, at this moment.
    ///
    /// The window most recently brought to the front, of the open ones that follow this machine.
    /// A tab made outside Muster - in herdr's own TUI, say - has nothing else to say where it
    /// belongs, and the window somebody was last looking at is where they will look for it. It
    /// is also the upgrade path: the first window to open after this record existed is the only
    /// one open, so it takes every tab, as a single window always did.
    ///
    /// Only windows following the machine are asked, because a window that cannot see a tab
    /// cannot show it, and waiting on one would leave the tab held by nobody.
    ///
    /// Unless a window is waiting on this machine. Then nobody takes it yet, because it is very
    /// likely that window's tab and the answer saying so is on its way. An expectation from a
    /// window that has closed, or older than [`Holders::EXPECTATION_LASTS`], is a request that
    /// is never going to be answered, and is ignored.
    pub fn taker(&self, daemon: &DaemonId, now: i64, open: impl Fn(&HeldWindow) -> bool) -> Taker {
        let waiting = self.expecting.iter().find(|expecting| {
            &expecting.daemon == daemon
                && now.saturating_sub(expecting.since) < Holders::EXPECTATION_LASTS
                && self.windows.get(&expecting.window).is_some_and(&open)
        });
        if let Some(expecting) = waiting {
            return Taker::Waiting(expecting.window.clone());
        }
        self.in_front(daemon, open).map_or(Taker::Nobody, |window| Taker::Window(window.clone()))
    }

    /// The open window following this machine that came to the front most recently.
    ///
    /// Who a tab nobody holds joins, and who speaks for a tab no open window holds: a blocked
    /// agent there is announced by the window somebody is most likely looking at, and by that
    /// one only, so two open windows do not both post it.
    pub fn in_front(
        &self,
        daemon: &DaemonId,
        open: impl Fn(&HeldWindow) -> bool,
    ) -> Option<&WindowName> {
        self.windows
            .values()
            .filter(|window| window.daemons.contains(daemon) && open(window))
            .max_by(|a, b| a.focused.cmp(&b.focused).then_with(|| b.name.cmp(&a.name)))
            .map(|window| &window.name)
    }

    /// How long a window may be waiting on an answer before its expectation is ignored.
    ///
    /// Thirty seconds, which is far beyond any answer a daemon gives - `pane new --run` waits on
    /// a shell and still answers in a few. Past it, the window that wrote it crashed or lost
    /// the answer, and a tab it would have taken should not wait forever on it.
    pub const EXPECTATION_LASTS: i64 = 30_000;
}

/// The version this format is on.
const VERSION: i64 = 1;

/// The record as the text that gets written to disk.
///
/// TOML, like every file beside it, and flat rows rather than a table per window: somebody who
/// opens this to find out why a tab is in the wrong window reads it top to bottom.
pub fn to_toml(holders: &Holders) -> String {
    let mut root = toml::Table::new();
    root.insert("version".to_string(), toml::Value::Integer(VERSION));

    let windows: Vec<toml::Value> = holders
        .windows
        .values()
        .map(|window| {
            let mut table = toml::Table::new();
            table.insert("name".to_string(), toml::Value::String(window.name.to_string()));
            table
                .insert("arrangement".to_string(), toml::Value::String(window.arrangement.clone()));
            table.insert("socket".to_string(), toml::Value::String(window.socket.clone()));
            table.insert("pid".to_string(), toml::Value::Integer(i64::from(window.pid)));
            table.insert("focused".to_string(), toml::Value::Integer(window.focused));
            table.insert(
                "daemons".to_string(),
                toml::Value::Array(
                    window
                        .daemons
                        .iter()
                        .map(|daemon| toml::Value::String(daemon.to_string()))
                        .collect(),
                ),
            );
            toml::Value::Table(table)
        })
        .collect();
    if !windows.is_empty() {
        root.insert("window".to_string(), toml::Value::Array(windows));
    }

    let tabs: Vec<toml::Value> = holders
        .tabs
        .iter()
        .map(|(tab, window)| {
            let mut table = toml::Table::new();
            table.insert("name".to_string(), toml::Value::String(tab.to_string()));
            table.insert("window".to_string(), toml::Value::String(window.to_string()));
            toml::Value::Table(table)
        })
        .collect();
    if !tabs.is_empty() {
        root.insert("tab".to_string(), toml::Value::Array(tabs));
    }

    let expecting: Vec<toml::Value> = holders
        .expecting
        .iter()
        .map(|expecting| {
            let mut table = toml::Table::new();
            table.insert("window".to_string(), toml::Value::String(expecting.window.to_string()));
            table.insert("daemon".to_string(), toml::Value::String(expecting.daemon.to_string()));
            table.insert("since".to_string(), toml::Value::Integer(expecting.since));
            toml::Value::Table(table)
        })
        .collect();
    if !expecting.is_empty() {
        root.insert("expecting".to_string(), toml::Value::Array(expecting));
    }

    toml::to_string_pretty(&toml::Value::Table(root))
        .unwrap_or_else(|error| panic!("who holds which tab should always render as TOML: {error}"))
}

/// Reads the record back, or says why it will not.
///
/// An empty record is an empty answer rather than a refusal: it is what the first window to
/// open after this existed finds, and it means nobody holds anything yet.
pub fn from_toml(text: &str) -> Result<Holders, String> {
    if text.trim().is_empty() {
        return Ok(Holders::new());
    }
    let root: toml::Table = toml::from_str(text).map_err(|error| {
        format!("the record of which window holds each tab is not TOML: {error}")
    })?;
    match root.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "the record of which window holds each tab is version {other} and this Muster \
                 writes version {VERSION}"
            ));
        }
        None => {
            return Err(
                "the record of which window holds each tab does not say what version it is"
                    .to_string(),
            );
        }
    }

    let mut holders = Holders::new();
    // A row that will not read is skipped rather than failing the record, on the same terms as a
    // saved region: one unreadable line costs one tab its window, and it joins another.
    for table in rows(&root, "window") {
        let Some(name) = text_at(table, "name") else { continue };
        holders.windows.insert(
            WindowName::new(name.clone()),
            HeldWindow {
                name: WindowName::new(name),
                arrangement: text_at(table, "arrangement").unwrap_or_default(),
                socket: text_at(table, "socket").unwrap_or_default(),
                pid: table
                    .get("pid")
                    .and_then(toml::Value::as_integer)
                    .and_then(|pid| u32::try_from(pid).ok())
                    .unwrap_or(0),
                focused: table.get("focused").and_then(toml::Value::as_integer).unwrap_or(0),
                daemons: table
                    .get("daemons")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .map(DaemonId::new)
                    .collect(),
            },
        );
    }
    for table in rows(&root, "tab") {
        let (Some(name), Some(window)) = (text_at(table, "name"), text_at(table, "window")) else {
            continue;
        };
        holders.take(TabId::new(name), &WindowName::new(window));
    }
    for table in rows(&root, "expecting") {
        let (Some(window), Some(daemon)) = (text_at(table, "window"), text_at(table, "daemon"))
        else {
            continue;
        };
        holders.expecting.push(Expecting {
            window: WindowName::new(window),
            daemon: DaemonId::new(daemon),
            since: table.get("since").and_then(toml::Value::as_integer).unwrap_or(0),
        });
    }
    Ok(holders)
}

fn rows<'a>(root: &'a toml::Table, key: &str) -> impl Iterator<Item = &'a toml::Table> {
    root.get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
}

fn text_at(table: &toml::Table, key: &str) -> Option<String> {
    table.get(key)?.as_str().filter(|value| !value.is_empty()).map(str::to_string)
}
