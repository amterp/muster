//! This window's side of the record of which window holds each tab.
//!
//! The rules are the core's (`muster_core::composition::holding`); what is here is the part
//! that needs a process: which window this is, the file every window shares, the clock, and
//! whether another window is open - which is asked by dialing its socket rather than trusting a
//! pid, because a pid outlives nothing and is handed to the next process that asks.

use std::collections::BTreeSet;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use muster_core::composition::holding::{from_toml, to_toml};
use muster_core::composition::{DaemonId, HeldWindow, Holders, Taker, WindowName};
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::intent::Refusal;
use muster_core::mirror::backend::TabId;
use muster_core::shared::SharedRecord;

use crate::shared_file::{HOLDERS, SharedFile};

/// Which window this is, and what it last read about which window holds each tab.
#[derive(Debug)]
pub(crate) struct Holding {
    me: WindowName,
    /// Where this window's arrangement is written, or empty for a window that remembers nothing.
    arrangement: String,
    /// Where this window answers requests, or empty for a window nothing outside can reach.
    socket: String,
    /// The file every window shares, or `None` for a window with nowhere to write one - which
    /// is every test that names none, and a window that then holds every tab, as a single window
    /// always did.
    record: Option<SharedFile>,
    /// The record as this window last read or wrote it.
    holders: Holders,
    /// Tabs nobody holds that this window has already decided are not its to take.
    ///
    /// Remembered so that deciding costs a dial per window once rather than on every reconcile:
    /// while a tab sits unclaimed because another window is in front or waiting on an answer,
    /// every agent transition reconciles. Forgotten whenever the record moves, because that is
    /// what changes the answer.
    passed_over: BTreeSet<TabId>,
}

impl Default for Holding {
    fn default() -> Holding {
        Holding::new("", "", "")
    }
}

impl Holding {
    /// This window, named after its arrangement: `window-2` for `.../windows/window-2.toml`.
    ///
    /// A window told to remember nothing has no arrangement to be named after, and is named
    /// after its process instead. It cannot be reopened, so it holds its tabs only while it is
    /// open - the core forgets a closed window with no arrangement.
    pub(crate) fn new(record: &str, arrangement: &str, socket: &str) -> Holding {
        let me = Path::new(arrangement)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| format!("pid-{}", std::process::id()));
        Holding {
            me: WindowName::new(me),
            arrangement: arrangement.to_string(),
            socket: socket.to_string(),
            record: (!record.is_empty()).then(|| SharedFile::at(record, &HOLDERS)),
            holders: Holders::new(),
            passed_over: BTreeSet::new(),
        }
    }

    pub(crate) fn me(&self) -> &WindowName {
        &self.me
    }

    pub(crate) fn holders(&self) -> &Holders {
        &self.holders
    }

    /// Whether this window holds a tab, as the record last said.
    pub(crate) fn holds(&self, tab: &TabId) -> bool {
        self.holders.holder(tab) == Some(&self.me)
    }

    /// The other window holding a tab, if one does.
    pub(crate) fn elsewhere(&self, tab: &TabId) -> Option<&HeldWindow> {
        let holder = self.holders.holder(tab).filter(|holder| **holder != self.me)?;
        self.holders.window(holder)
    }

    /// Says this window is open, and settles what the record says about windows that are not.
    ///
    /// `known` is whether a tab's name still resolves anywhere, which is how rows for tabs long
    /// gone stop accumulating. Once per launch is enough for that, and it is the one moment
    /// every window reliably passes through.
    pub(crate) fn open(&mut self, known: impl Fn(&TabId) -> bool) {
        let me = self.me.clone();
        let window = HeldWindow {
            name: me.clone(),
            arrangement: self.arrangement.clone(),
            socket: self.socket.clone(),
            pid: std::process::id(),
            focused: now(),
        };
        self.change(move |holders| {
            // Before registering this window, so the question is about the others. A window
            // whose arrangement is gone can never be reopened, and a tab it held would otherwise
            // be held by nobody who could ever show it.
            holders.forget(|window| {
                window.name != me
                    && !is_open(&me, window)
                    && (window.arrangement.is_empty() || !Path::new(&window.arrangement).exists())
            });
            holders.opened(window);
            holders.prune(known);
        });
    }

    /// Says this window has closed. It keeps its tabs, so reopening it comes back to them.
    pub(crate) fn close(&mut self) {
        let me = self.me.clone();
        self.change(|holders| holders.closed(&me));
    }

    /// Says this window has come to the front, which makes it the one a tab nobody holds joins.
    pub(crate) fn focused(&mut self) {
        let me = self.me.clone();
        self.change(|holders| holders.focused(&me, now()));
    }

    /// Says this window is about to ask a machine for a tab.
    pub(crate) fn expect(&mut self, daemon: &DaemonId) {
        let me = self.me.clone();
        self.change(|holders| holders.expect(&me, daemon, now()));
    }

    /// Says the answer came back, and takes the tab it named, if it named one.
    ///
    /// One write for both, so no window ever reads a record where this window has stopped
    /// waiting and has not yet taken what it was waiting for.
    pub(crate) fn answered(&mut self, daemon: &DaemonId, tab: Option<&TabId>) {
        let me = self.me.clone();
        self.change(|holders| {
            if let Some(tab) = tab {
                holders.take(tab.clone(), &me);
            }
            holders.expected(&me, daemon);
        });
    }

    /// Takes the tabs among these that nobody holds, in one write.
    ///
    /// What a window reopening does with the tabs its arrangement lists. One another window has
    /// taken since is left where it is.
    pub(crate) fn keep(&mut self, listed: &[TabId]) {
        if listed.iter().all(|tab| self.holders.holder(tab).is_some()) {
            return;
        }
        let me = self.me.clone();
        self.change(|holders| {
            for tab in listed {
                if holders.holder(tab).is_none() {
                    holders.take(tab.clone(), &me);
                }
            }
        });
    }

    /// Gives a tab to a window, this one or another, open or closed.
    pub(crate) fn give(&mut self, tab: &TabId, window: &WindowName) {
        self.change(|holders| holders.take(tab.clone(), window));
    }

    /// The window a caller means: this one for nothing, an open window for a pid, or a window by
    /// name, open or closed.
    ///
    /// A pid names only an open window, because a closed window has no process - and a pid whose
    /// window has gone is a number the next process may already have.
    pub(crate) fn destination(&self, said: &str) -> Result<WindowName, Refusal> {
        if said.is_empty() {
            return Ok(self.me.clone());
        }
        if let Ok(pid) = said.parse::<u32>() {
            if pid == std::process::id() {
                return Ok(self.me.clone());
            }
            return self
                .holders
                .windows()
                .find(|window| window.pid == pid && is_open(&self.me, window))
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
        if name == self.me || self.holders.window(&name).is_some() {
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

    /// Takes the tabs nobody holds on this machine, if this window is the one they join.
    ///
    /// Answers with what was taken. Checked again under the hold, because another window may
    /// have taken one between this window reading the record and deciding.
    pub(crate) fn take_unheld(&mut self, daemon: &DaemonId, described: &[TabId]) -> Vec<TabId> {
        let unheld: Vec<TabId> = described
            .iter()
            .filter(|tab| self.holders.holder(tab).is_none() && !self.passed_over.contains(*tab))
            .cloned()
            .collect();
        if unheld.is_empty() {
            return Vec::new();
        }
        let me = self.me.clone();
        // With nowhere to share the record there is no other window to hand a tab to, so this
        // one holds every tab, as a single window always did.
        if self.record.is_none() {
            for tab in &unheld {
                self.holders.take(tab.clone(), &me);
            }
            return unheld;
        }
        match self.holders.taker(daemon, now(), |window| is_open(&me, window)) {
            Taker::Window(taker) if taker == me => {}
            other => {
                log::debug(
                    "holding.passed_over",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "tabs" => join(&unheld),
                        "taker" => match other {
                            Taker::Window(window) => window.to_string(),
                            Taker::Waiting(window) => format!("waiting on {window}"),
                            Taker::Nobody => "nobody".to_string(),
                        },
                    },
                );
                self.passed_over.extend(unheld);
                return Vec::new();
            }
        }
        self.change(|holders| {
            for tab in &unheld {
                if holders.holder(tab).is_none() {
                    holders.take(tab.clone(), &me);
                }
            }
        });
        let taken: Vec<TabId> = unheld.into_iter().filter(|tab| self.holds(tab)).collect();
        if !taken.is_empty() {
            log::info(
                "holding.taken",
                fields! { "daemon" => daemon.to_string(), "tabs" => join(&taken) },
            );
        }
        taken
    }

    /// Reads the record again, and says whether anything this window acts on moved.
    ///
    /// A window coming to the front changes the record too, and on its own that moves nothing
    /// on screen - so only who holds which tab counts as a change.
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
        let moved = holders
            .windows()
            .any(|window| holders.held_by(&window.name).ne(self.holders.held_by(&window.name)))
            || self
                .holders
                .windows()
                .any(|window| holders.held_by(&window.name).ne(self.holders.held_by(&window.name)));
        self.holders = holders;
        self.passed_over.clear();
        moved
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
        record.exclusively(&mut |text| {
            let mut holders = match from_toml(text) {
                Ok(holders) => holders,
                Err(detail) => {
                    unreadable(&detail);
                    fallback.clone()
                }
            };
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

/// Whether a window is open: this one always is, and another is when its socket answers.
///
/// A connect and nothing else. The window at the other end reads no request and logs that at
/// debug, which is the cost of asking; a socket file left behind by a window that crashed
/// refuses, which is the answer.
pub(crate) fn is_open(me: &WindowName, window: &HeldWindow) -> bool {
    &window.name == me || (!window.socket.is_empty() && UnixStream::connect(&window.socket).is_ok())
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
                        the file; deleting it costs only which window each tab was in, and every \
                        tab then joins the window in front",
        },
    );
}
