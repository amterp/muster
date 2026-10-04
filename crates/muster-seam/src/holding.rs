//! This process's side of the record of which window holds each tab.
//!
//! The rules are the core's (`muster_core::composition::holding`); what is here is the part
//! that needs a process: which windows are this process's, the file the record is kept in, and
//! the clock.
//!
//! Every window of an install is a window of this one process, and no second process of the
//! install can run beside it (mip/0006-one-process.md, section 5). So the record is this
//! process's alone: read once at startup, written whole whenever it changes, and kept for the
//! next launch to read. Nothing else writes it while this process runs, so nothing is locked and
//! nothing is read back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, Taker, WindowName};
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::intent::Refusal;
use muster_core::mirror::backend::TabId;
use muster_daemon_proto::install;

/// This process's windows, and what it last read about which window holds each tab.
#[derive(Debug)]
pub(crate) struct Holding {
    /// Where this process answers requests, or empty for one nothing outside can reach. Every
    /// window here answers on it.
    socket: String,
    /// The file the record is kept in, or `None` for a process with nowhere to write one - which
    /// is every test that names none. Its windows then share every tab among themselves, and a
    /// process with one window holds every tab, as a single window always did.
    record: Option<PathBuf>,
    /// The record, as this process has it: read at startup and changed here since.
    holders: Holders,
    /// The machines this process follows, as it last wrote them into its windows' rows. Every
    /// window here follows the same ones.
    daemons: BTreeSet<DaemonId>,
    /// Closed windows this process has asked the shell to reopen, and when.
    ///
    /// Remembered so that two requests close together - a double click on a notification - ask
    /// once. The second arrives before the shell has opened the window and said so.
    reopening: BTreeMap<WindowName, Instant>,
    /// The windows this process shows, by name.
    here: BTreeMap<WindowName, Here>,
}

/// One of this process's windows, as the record needs it.
#[derive(Debug, Clone)]
struct Here {
    /// Where its arrangement is written, or empty for a window that remembers nothing.
    arrangement: String,
    /// Whether it has said it is open and not yet said it closed. While it is, a record that
    /// has lost its row is given it back.
    open: bool,
    /// Whether it has said it closed. Until it does it counts as open to everything asking
    /// whether a window is, as a window always has itself, including in the moment before it
    /// has said so.
    closed: bool,
}

impl Default for Holding {
    fn default() -> Holding {
        let mut holding = Holding::new("", "");
        holding.register("");
        holding
    }
}

impl Holding {
    pub(crate) fn new(record: &str, socket: &str) -> Holding {
        let record = (!record.is_empty()).then(|| PathBuf::from(record));
        Holding {
            socket: socket.to_string(),
            holders: record.as_deref().map(read).unwrap_or_default(),
            record,
            daemons: BTreeSet::new(),
            reopening: BTreeMap::new(),
            here: BTreeMap::new(),
        }
    }

    /// Takes on a window of this process, and answers what it is called: `window-2` for
    /// `.../windows/window-2.toml`.
    ///
    /// A window told to remember nothing has no arrangement to be named after, and is named
    /// after its process instead. It cannot be reopened, so it holds its tabs only while it is
    /// open - the core forgets a closed window with no arrangement.
    pub(crate) fn register(&mut self, arrangement: &str) -> WindowName {
        let name = Path::new(arrangement)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .map_or_else(|| self.unnamed(), WindowName::new);
        self.here.insert(
            name.clone(),
            Here { arrangement: arrangement.to_string(), open: false, closed: false },
        );
        name
    }

    /// A name for a window with no arrangement, after its process: `pid-4321`, then
    /// `pid-4321-2` for a second one.
    fn unnamed(&self) -> WindowName {
        unnamed(self.here.keys().filter(|name| name.as_str().starts_with("pid-")).count())
    }

    /// This process's windows that are open: every one that has not said it closed. Every open
    /// window of the install is one of these.
    pub(crate) fn open_here(&self) -> BTreeSet<WindowName> {
        self.here.iter().filter(|(_, here)| !here.closed).map(|(name, _)| name.clone()).collect()
    }

    /// This process's windows that have said they are open, which are the rows it writes.
    fn said_open(&self) -> BTreeSet<WindowName> {
        self.here.iter().filter(|(_, here)| here.open).map(|(name, _)| name.clone()).collect()
    }

    pub(crate) fn holders(&self) -> &Holders {
        &self.holders
    }

    /// Whether a window holds a tab, as the record last said.
    pub(crate) fn holds(&self, window: &WindowName, tab: &TabId) -> bool {
        self.holders.holder(tab) == Some(window)
    }

    /// The window holding a tab other than `me`, if one does.
    pub(crate) fn elsewhere(&self, me: &WindowName, tab: &TabId) -> Option<&HeldWindow> {
        let holder = self.holders.holder(tab).filter(|holder| *holder != me)?;
        self.holders.window(holder)
    }

    /// Says a window is open, and settles what the record says about windows that are not.
    ///
    /// `answered` and `described` are what the machines this process follows hold, which is how
    /// rows for tabs long gone stop accumulating (`Holders::prune`). Once per launch is enough
    /// for that, and it is the one moment every window reliably passes through.
    pub(crate) fn open(
        &mut self,
        me: &WindowName,
        answered: &BTreeSet<DaemonId>,
        described: impl Fn(&TabId) -> bool,
    ) {
        if !self.here.contains_key(me) {
            return;
        }
        let window = self.row(me, now());
        let mine = self.here.clone();
        self.change(move |holders| {
            // Before registering this window, so the question is about the others. A window
            // whose arrangement is gone can never be reopened, and a tab it held would otherwise
            // be held by nobody who could ever show it.
            holders.forget(|window| {
                !mine.contains_key(&window.name)
                    && (window.arrangement.is_empty() || !Path::new(&window.arrangement).exists())
            });
            holders.opened(window);
            holders.prune(answered, described);
        });
        if let Some(here) = self.here.get_mut(me) {
            here.open = true;
            here.closed = false;
        }
        // It is open, so a request to reopen it has been answered: the next one, after it closes
        // again, is a new request rather than the same one repeated.
        self.reopening.remove(me);
    }

    /// A window's row in the record.
    fn row(&self, me: &WindowName, focused: i64) -> HeldWindow {
        HeldWindow {
            name: me.clone(),
            arrangement: self.here.get(me).map(|here| here.arrangement.clone()).unwrap_or_default(),
            socket: self.socket.clone(),
            pid: std::process::id(),
            install: install::INSTALL.to_string(),
            focused,
            daemons: self.daemons.clone(),
        }
    }

    /// Whether this process's rows already name exactly these machines.
    pub(crate) fn follows_exactly<'a>(&self, daemons: impl Iterator<Item = &'a DaemonId>) -> bool {
        self.daemons.iter().eq(daemons)
    }

    /// Says which machines this process follows, so that a tab on one it does not follow is left
    /// to a window that does. Written into the rows of the windows that are open; a window not
    /// open yet writes it with the rest of its row.
    pub(crate) fn follow(&mut self, daemons: BTreeSet<DaemonId>) {
        self.daemons = daemons;
        let open = self.said_open();
        if !open.is_empty() {
            let daemons = self.daemons.clone();
            self.change(|holders| {
                for me in &open {
                    holders.follows(me, daemons.clone());
                }
            });
        }
    }

    /// Whether a window has said it is open, and not since said it closed.
    pub(crate) fn has_opened(&self, me: &WindowName) -> bool {
        self.here.get(me).is_some_and(|here| here.open)
    }

    /// Says a window has closed. It keeps its tabs, so reopening it comes back to them.
    pub(crate) fn close(&mut self, me: &WindowName) {
        if let Some(here) = self.here.get_mut(me) {
            here.open = false;
            here.closed = true;
        }
        self.change(|holders| holders.closed(me));
    }

    /// Says a window has come to the front, which makes it the one a tab nobody holds joins.
    pub(crate) fn focused(&mut self, me: &WindowName) {
        self.change(|holders| holders.focused(me, now()));
    }

    /// Says this process is asking for a closed window back, and whether it had not already.
    ///
    /// Ten seconds is far longer than the shell takes to open a window, which then says it is open
    /// and is asked nothing more.
    pub(crate) fn ask_to_reopen(&mut self, window: &WindowName) -> bool {
        const STARTING: Duration = Duration::from_secs(10);
        let now = Instant::now();
        self.reopening.retain(|_, asked| now.duration_since(*asked) < STARTING);
        if self.reopening.contains_key(window) {
            return false;
        }
        self.reopening.insert(window.clone(), now);
        true
    }

    /// Takes a tab a window is about to ask a daemon to make.
    ///
    /// Before the request rather than after its answer: the daemon announces the tab before it
    /// answers, and every window hears it, so a tab nobody held in that moment would go to
    /// whichever window came to the front last.
    pub(crate) fn making(&mut self, me: &WindowName, tab: &TabId) {
        self.change(|holders| holders.take(tab.clone(), me));
    }

    /// Takes the tabs among these that nobody holds for a window, in one write.
    ///
    /// What a window reopening does with the tabs its arrangement lists. One another window has
    /// taken since is left where it is.
    pub(crate) fn keep(&mut self, me: &WindowName, listed: &[TabId]) {
        if listed.iter().all(|tab| self.holders.holder(tab).is_some()) {
            return;
        }
        self.change(|holders| {
            for tab in listed {
                if holders.holder(tab).is_none() {
                    holders.take(tab.clone(), me);
                }
            }
        });
    }

    /// Gives a tab to a window, this one or another, open or closed.
    pub(crate) fn give(&mut self, tab: &TabId, window: &WindowName) {
        self.change(|holders| holders.take(tab.clone(), window));
    }

    /// The window a caller means: `me` for nothing, the window in front for this app's pid, or a
    /// window by name, open or closed.
    ///
    /// A pid names only an open window, because a closed window has no process - and a pid whose
    /// window has gone is a number the next process may already have. Every open window is this
    /// process's, so only this process's own pid names any of them, and it means the one in front
    /// (mip/0006-one-process.md, Compatibility): what a pid meant when each window was a process.
    pub(crate) fn destination(
        &self,
        me: &WindowName,
        front: &WindowName,
        said: &str,
    ) -> Result<WindowName, Refusal> {
        if said.is_empty() {
            return Ok(me.clone());
        }
        if let Ok(pid) = said.parse::<u32>() {
            if pid == std::process::id() {
                return Ok(front.clone());
            }
            return Err(Refusal::NotThere(format!(
                "no open window has pid {pid}, so nothing was moved. `muster window list` shows \
                 the windows that are open, and a closed one is named rather than numbered: {}.",
                self.known()
            )));
        }
        let name = WindowName::new(said);
        if self.here.contains_key(&name) || self.holders.window(&name).is_some() {
            return Ok(name);
        }
        Err(Refusal::NotThere(format!(
            "no window is called {said}, so nothing was moved. The windows there are: {}.",
            self.known()
        )))
    }

    /// Every window the record knows, as a person would name each.
    fn known(&self) -> String {
        let open_here = self.open_here();
        self.holders
            .windows()
            .map(|window| {
                if open_here.contains(&window.name) {
                    window.name.to_string()
                } else {
                    format!("{} (closed)", window.name)
                }
            })
            .collect::<Vec<String>>()
            .join(", ")
    }

    /// Takes the tabs nobody holds on this machine, if one of this process's windows is the one
    /// they join, for that window.
    ///
    /// Answers with what was taken; the record says by which window.
    pub(crate) fn take_unheld(&mut self, daemon: &DaemonId, described: &[TabId]) -> Vec<TabId> {
        let unheld: Vec<TabId> =
            described.iter().filter(|tab| self.holders.holder(tab).is_none()).cloned().collect();
        if unheld.is_empty() {
            return Vec::new();
        }
        let open_here = self.open_here();
        let joins = match self.holders.taker(daemon, |window| open_here.contains(&window.name)) {
            Taker::Window(taker) if self.here.contains_key(&taker) => taker,
            // With nowhere to keep the record, one of this process's windows holds it whether or
            // not any has said it is open: the one in front, or failing that the first by name. A
            // single window holds every tab, as it always did.
            _ if self.record.is_none() => match self.here.keys().next() {
                Some(first) => first.clone(),
                None => return Vec::new(),
            },
            other => {
                log::debug(
                    "holding.passed_over",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "tabs" => join(&unheld),
                        "taker" => match other {
                            Taker::Window(window) => window.to_string(),
                            Taker::Nobody => "nobody".to_string(),
                        },
                    },
                );
                return Vec::new();
            }
        };
        self.change(|holders| {
            for tab in &unheld {
                if holders.holder(tab).is_none() {
                    holders.take(tab.clone(), &joins);
                }
            }
        });
        let taken: Vec<TabId> = unheld.into_iter().filter(|tab| self.holds(&joins, tab)).collect();
        if !taken.is_empty() {
            log::info(
                "holding.taken",
                fields! {
                    "daemon" => daemon.to_string(),
                    "window" => joins.to_string(),
                    "tabs" => join(&taken),
                },
            );
        }
        taken
    }

    /// Changes the record, and writes it.
    fn change(&mut self, work: impl FnOnce(&mut Holders)) {
        work(&mut self.holders);
        if let Some(record) = &self.record {
            write(record, &self.holders);
        }
    }
}

/// What a window with no arrangement is called, the `nth` such window of this process counting
/// from zero.
fn unnamed(nth: usize) -> WindowName {
    match nth {
        0 => WindowName::new(format!("pid-{}", std::process::id())),
        nth => WindowName::new(format!("pid-{}-{}", std::process::id(), nth + 1)),
    }
}

/// The name of the first window of a process that has registered none, which is what a session
/// nobody has started yet calls its window.
pub(crate) fn first_unnamed() -> WindowName {
    unnamed(0)
}

/// The arrangements of this install's windows that were open when Muster last ended, for the
/// launch to open again, focused longest ago first.
///
/// Read before any session exists, which is why it takes the record's path rather than a
/// `Holding`: the answer decides what the launch tells the core about its first window. A record
/// that cannot be read is nothing to reopen, and the launch opens one window as it always did.
pub(crate) fn reopening(record: &str) -> Vec<String> {
    if record.is_empty() {
        return Vec::new();
    }
    // Every row the last process of this install left open is a window open when it ended: this
    // launch holds the app, so no process of the install is still running to be showing one.
    read(Path::new(record))
        .open_when_last_ended(install::INSTALL, |arrangement| Path::new(arrangement).exists())
}

/// The record as the file holds it: empty when there is no file yet, and empty with a warning
/// when this build cannot read it.
fn read(record: &Path) -> Holders {
    match std::fs::read_to_string(record) {
        Ok(text) => from_toml(&text).unwrap_or_else(|detail| {
            unreadable(&detail);
            Holders::new()
        }),
        Err(_) => Holders::new(),
    }
}

/// Writes the record whole, through a file beside it renamed into place, so the next launch never
/// reads one half written.
fn write(record: &Path, holders: &Holders) {
    let staged = record.with_extension("writing");
    let written = record
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&staged, to_toml(holders)))
        .and_then(|()| std::fs::rename(&staged, record));
    if let Err(failure) = written {
        log::warn(
            "holding.save.failed",
            fields! {
                "record" => record.display(),
                "detail" => failure.to_string(),
                "impact" => "which window holds each tab is right while Muster runs, but the next \
                             launch reads what was last written: it may reopen a window's tabs in \
                             another, or not reopen a window that was open",
                "check" => "whether the state directory is writable and the disk has room",
            },
        );
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

fn join(tabs: &[TabId]) -> String {
    tabs.iter().map(TabId::as_str).collect::<Vec<&str>>().join(",")
}

fn unreadable(detail: &str) {
    log::warn(
        "holding.unreadable",
        fields! {
            "detail" => detail,
            "impact" => "the record of which window holds each tab was read as empty, so no \
                         window reopens from it and every tab joins the window in front; it is \
                         written afresh at the next change",
            "check" => "whether a newer Muster wrote it, and what is in the file; deleting it is \
                        safe, and costs only which closed window held which tab",
        },
    );
}
