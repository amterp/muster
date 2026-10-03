//! This process's side of the record of which window holds each tab.
//!
//! The rules are the core's (`muster_core::composition::holding`); what is here is the part
//! that needs a process: which windows are this process's, the file every window shares, the
//! clock, and whether a window in another process is open - which is asked by dialing its socket
//! rather than trusting a pid, because a pid outlives nothing and is handed to the next process
//! that asks.
//!
//! A process may show several windows (MIP-6), so the record and what was last read of it are
//! the process's, and each window is a row it writes for itself.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, Taker, WindowName};
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::intent::Refusal;
use muster_core::mirror::backend::TabId;
use muster_core::shared::SharedRecord;
use muster_daemon_proto::install;

use crate::shared_file::{HOLDERS, SharedFile};

/// This process's windows, and what it last read about which window holds each tab.
#[derive(Debug)]
pub(crate) struct Holding {
    /// Where this process answers requests, or empty for one nothing outside can reach. Every
    /// window here answers on it.
    socket: String,
    /// The file every window shares, or `None` for a process with nowhere to write one - which
    /// is every test that names none. Its windows then share every tab among themselves, and a
    /// process with one window holds every tab, as a single window always did.
    record: Option<SharedFile>,
    /// The record as this process last read or wrote it.
    holders: Holders,
    /// Tabs nobody holds that this process has already decided are not its windows' to take.
    ///
    /// Remembered so that deciding costs a dial per window once rather than on every reconcile:
    /// while a tab sits unclaimed because another window is in front or waiting on an answer,
    /// every agent transition reconciles. Forgotten whenever the record moves, because that is
    /// what changes the answer.
    passed_over: BTreeSet<TabId>,
    /// The machines this process follows, as it last wrote them into its windows' rows. Every
    /// window here follows the same ones.
    daemons: BTreeSet<DaemonId>,
    /// Closed windows this process has asked the shell to reopen, and when.
    ///
    /// Remembered so that two requests close together launch one app. The second arrives while
    /// the first launch is still starting, when the window does not answer on its socket yet.
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
        Holding {
            socket: socket.to_string(),
            record: (!record.is_empty()).then(|| SharedFile::at(record, &HOLDERS)),
            holders: Holders::new(),
            passed_over: BTreeSet::new(),
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

    /// This process's windows that are open, which are open without dialing anything: every one
    /// that has not said it closed.
    pub(crate) fn open_here(&self) -> BTreeSet<WindowName> {
        self.here.iter().filter(|(_, here)| !here.closed).map(|(name, _)| name.clone()).collect()
    }

    /// This process's windows that have said they are open, which are the rows it writes.
    fn said_open(&self) -> BTreeSet<WindowName> {
        self.here.iter().filter(|(_, here)| here.open).map(|(name, _)| name.clone()).collect()
    }

    /// Whether there is a record other windows read, rather than this process being the only one.
    pub(crate) fn is_shared(&self) -> bool {
        self.record.is_some()
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
        let (mine, open_here) = (self.here.clone(), self.open_here());
        self.change(move |holders| {
            // Before registering this window, so the question is about the others. A window
            // whose arrangement is gone can never be reopened, and a tab it held would otherwise
            // be held by nobody who could ever show it.
            holders.forget(|window| {
                !mine.contains_key(&window.name)
                    && !is_open(&open_here, window)
                    && (window.arrangement.is_empty() || !Path::new(&window.arrangement).exists())
            });
            holders.opened(window);
            holders.prune(answered, described);
        });
        // After the write rather than before, because the write is what gives this window its row:
        // marked open first, it would read as an open window the record had lost, and be put back.
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
    /// Ten seconds covers a window starting, after which it answers on its socket and a request
    /// for one of its tabs is carried to it instead of reaching here.
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

    /// The window a caller means: `me` for nothing, an open window for a pid, or a window by
    /// name, open or closed.
    ///
    /// A pid names only an open window, because a closed window has no process - and a pid whose
    /// window has gone is a number the next process may already have. This process's own pid
    /// means `me`, which a window in another process naming this one by pid means as well.
    pub(crate) fn destination(&self, me: &WindowName, said: &str) -> Result<WindowName, Refusal> {
        if said.is_empty() {
            return Ok(me.clone());
        }
        if let Ok(pid) = said.parse::<u32>() {
            if pid == std::process::id() {
                // This process's pid names one window only while it has one open. With several,
                // which of them is a guess, and a tab moved on a guess lands where nobody asked.
                let here = self.said_open();
                if here.len() > 1 {
                    let names: Vec<String> = here.iter().map(ToString::to_string).collect();
                    return Err(Refusal::Declined(format!(
                        "pid {pid} is a Muster with {} windows open, so it does not say which one, \
                         and nothing was moved. Name the window instead: {}.",
                        here.len(),
                        names.join(", ")
                    )));
                }
                return Ok(me.clone());
            }
            let open_here = self.open_here();
            return self
                .holders
                .windows()
                .find(|window| window.pid == pid && is_open(&open_here, window))
                .map(|window| window.name.clone())
                .ok_or_else(|| {
                    Refusal::NotThere(format!(
                        "no open window has pid {pid}, so nothing was moved. `muster window list` \
                         shows the windows that are open, and a closed one is named rather than \
                         numbered: {}.",
                        self.known()
                    ))
                });
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
        self.holders
            .windows()
            .map(|window| match window.pid {
                0 => format!("{} (closed)", window.name),
                pid => format!("{} (pid {pid})", window.name),
            })
            .collect::<Vec<String>>()
            .join(", ")
    }

    /// Takes the tabs nobody holds on this machine, if one of this process's windows is the one
    /// they join, for that window.
    ///
    /// Answers with what was taken; the record says by which window. Checked again under the
    /// hold, because another window may have taken one between this process reading the record
    /// and deciding.
    pub(crate) fn take_unheld(&mut self, daemon: &DaemonId, described: &[TabId]) -> Vec<TabId> {
        let unheld: Vec<TabId> = described
            .iter()
            .filter(|tab| self.holders.holder(tab).is_none() && !self.passed_over.contains(*tab))
            .cloned()
            .collect();
        if unheld.is_empty() {
            return Vec::new();
        }
        let open_here = self.open_here();
        let joins = match self.holders.taker(daemon, |window| is_open(&open_here, window)) {
            Taker::Window(taker) if self.here.contains_key(&taker) => taker,
            // With nowhere to share the record there is no other process to hand a tab to, so
            // one of this process's windows holds it whether or not any has said it is open: the
            // one in front, or failing that the first by name. A single window holds every tab, as
            // it always did.
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
                self.passed_over.extend(unheld);
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

    /// Reads the record again, and says whether who holds which tab moved.
    pub(crate) fn reread(&mut self) -> bool {
        let Some(record) = &self.record else { return false };
        let mut read = None;
        record.exclusively(&mut |text| {
            read = Some(text.to_string());
            None
        });
        let Some(text) = read else { return false };
        let holders = match from_toml(&text) {
            Ok(holders) => holders,
            Err(detail) => {
                unreadable(&detail);
                return false;
            }
        };
        let before = self.holders.clone();
        if self.said_open().iter().any(|me| holders.window(me).is_none()) {
            // A write rather than an edit of what was just read, so it happens under the hold and
            // `change` puts this process's windows back from the copy in memory.
            self.change(|_| {});
        } else {
            self.holders = holders;
        }
        self.passed_over.clear();
        moved(&before, &self.holders)
    }

    /// Reads the record, changes it, and writes it back, all inside one hold.
    fn change(&mut self, work: impl FnOnce(&mut Holders)) {
        self.passed_over.clear();
        let Some(record) = &self.record else {
            work(&mut self.holders);
            return;
        };
        let mut pending = Some(work);
        let mut changed = None;
        let fallback = self.holders.clone();
        let rows: Vec<HeldWindow> = self
            .said_open()
            .iter()
            .map(|me| {
                self.row(me, self.holders.window(me).map_or_else(now, |window| window.focused))
            })
            .collect();
        record.exclusively(&mut |text| {
            let mut holders = match from_toml(text) {
                Ok(holders) => holders,
                Err(detail) => {
                    unreadable(&detail);
                    fallback.clone()
                }
            };
            for row in &rows {
                if holders.window(&row.name).is_none() {
                    rejoin(&mut holders, &fallback, row.clone());
                }
            }
            let work = pending.take().expect("a hold does its work once");
            work(&mut holders);
            let written = to_toml(&holders);
            changed = Some(holders);
            Some(written)
        });
        if let Some(holders) = changed {
            self.holders = holders;
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

/// Whether who holds which tab differs between two copies of the record.
///
/// A window coming to the front changes the record too, and on its own that moves nothing on
/// screen - so only who holds which tab counts.
fn moved(before: &Holders, after: &Holders) -> bool {
    let differs =
        |window: &HeldWindow| after.held_by(&window.name).ne(before.held_by(&window.name));
    after.windows().any(differs) || before.windows().any(differs)
}

/// Puts an open window back into a record that has lost it, with the tabs it held that no
/// window has taken since.
///
/// A record loses an open window two ways: somebody deleted the file, which the warning for an
/// unreadable one invites, or another window opening forgot this one because its socket did not
/// answer. Either way this window is still showing those tabs, and without its row it would let
/// go of every one of them.
fn rejoin(holders: &mut Holders, remembered: &Holders, window: HeldWindow) {
    let me = &window.name.clone();
    holders.opened(window);
    let mut taken = Vec::new();
    for tab in remembered.held_by(me) {
        if holders.holder(tab).is_none() {
            holders.take(tab.clone(), me);
            taken.push(tab.clone());
        }
    }
    log::warn(
        "holding.rejoined",
        fields! {
            "window" => me.to_string(),
            "tabs" => join(&taken),
            "impact" => "the record had lost this open window, so it was written back with the \
                         tabs it held; a tab another window took in the meantime stays there",
            "check" => "whether somebody deleted the record, or whether this window stopped \
                        answering on its socket long enough for another window to forget it",
        },
    );
}

/// Whether a window is open: one of this process's is when `open_here` says so, and another is
/// when its socket answers.
///
/// A connect and nothing else. The window at the other end reads no request and logs that at
/// debug, which is the cost of asking; a socket file left behind by a window that crashed
/// refuses, which is the answer.
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
    let mut read = None;
    SharedFile::at(record, &HOLDERS).exclusively(&mut |text| {
        read = Some(text.to_string());
        None
    });
    let holders = match from_toml(&read.unwrap_or_default()) {
        Ok(holders) => holders,
        Err(detail) => {
            unreadable(&detail);
            return Vec::new();
        }
    };
    holders.open_when_last_ended(
        install::INSTALL,
        |window| is_open(&BTreeSet::new(), window),
        |arrangement| Path::new(arrangement).exists(),
    )
}

pub(crate) fn is_open(open_here: &BTreeSet<WindowName>, window: &HeldWindow) -> bool {
    open_here.contains(&window.name)
        || (!window.socket.is_empty() && UnixStream::connect(&window.socket).is_ok())
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
            "impact" => "this window carries on from what it last read, so it keeps its own tabs, \
                         but it cannot see which tabs other windows have taken or given away \
                         since - it may list one of theirs, or miss one given to it",
            "check" => "whether another Muster of a different version is open, and what is in \
                        the file; deleting it is safe, since every open window writes itself and \
                        its tabs back, and costs only which closed window held which tab",
        },
    );
}
