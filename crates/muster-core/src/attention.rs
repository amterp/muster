//! Which agents have been seen, and which are still waiting to be.
//!
//! Four of the five agent states are the daemon's word and arrive as themselves. `done` is
//! `idle` on a pane nobody has looked at since its agent finished, and the two halves of that
//! have different owners. The daemon knows when an agent finished, and holds it on the pane's
//! record as `finished_unseen` for as long as it runs, so a window opened after the finish still
//! hears it. Only a window knows whether somebody looked, because only the shell can see its own
//! focus. So a window that shows a finished pane while it has the keyboard tells the pane's
//! daemon with `Seen`, the daemon clears the fact, and every window then paints the pane `idle`.
//!
//! This module decides when a window has seen a pane, and paints it seen at once rather than
//! after the daemon's answer: a pane somebody is looking at that stayed `done` for a round trip
//! would be the border contradicting the person reading it.
//!
//! **And which of them are worth interrupting somebody for.** Glanceable states are the
//! floor rather than the ceiling: a pane no region is showing is exactly the pane most
//! likely to be waiting, and a border it is not drawing tells nobody. So this also holds
//! the set of panes currently asking for a person, and decides when one joins or leaves it.
//! That is the split `architecture.md` draws, where the core owns the unread set and the
//! shell only delivers: what a banner says and how it is posted is an OS question, and is
//! not here.
//!
//! Pure - no clock, no window, no socket. It is a fold over what each pane's daemon says and
//! what the window was showing at the time, which is exactly what a recorded case can drive.

use std::collections::{BTreeMap, BTreeSet};

use crate::AgentState;
use crate::composition::{DaemonId, PaneKey};

/// What a pane is asking of the person, when it is asking anything.
///
/// Two of the states ask, and they ask the same question from opposite ends: `blocked` is an
/// agent that has stopped and wants an answer, and `done` is an agent that has stopped and
/// nobody has noticed. Neither `working`, `waiting` nor `idle` asks for anybody, and `unknown`
/// is the absence of an answer rather than one. The third is not a state: a program in the pane
/// asked for a notification, in words of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Alert {
    /// An agent waiting on somebody. First, because it is the one somebody is holding up.
    Blocked,
    /// A program asked to tell somebody something, and said what ([`Note`]).
    Notified,
    /// An agent that finished while nobody was looking.
    Done,
}

impl Alert {
    pub fn as_str(self) -> &'static str {
        match self {
            Alert::Blocked => "blocked",
            Alert::Notified => "notified",
            Alert::Done => "done",
        }
    }
}

/// What a program's notification said (OSC 9 or OSC 777).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub body: String,
}

/// Which of those are worth interrupting somebody for.
///
/// Both on by default, because both are a person being waited on and a state that never
/// notifies is a state you have to go and look for. The mute is the answer for fifteen
/// agents at once, and it is a third key rather than "set both to false" so that turning
/// the noise off for an afternoon does not cost you the two answers underneath it.
///
/// A program's own notification is on by default for the same reason: a program that asks to
/// notify somebody has said it is worth it, which a bell never has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "one answer per key of `[notifications]`")]
pub struct Notifications {
    pub blocked: bool,
    pub programs: bool,
    pub done: bool,
    pub muted: bool,
}

impl Default for Notifications {
    fn default() -> Notifications {
        Notifications { blocked: true, programs: true, done: true, muted: false }
    }
}

impl Notifications {
    fn allows(self, alert: Alert) -> bool {
        !self.muted
            && match alert {
                Alert::Blocked => self.blocked,
                Alert::Notified => self.programs,
                Alert::Done => self.done,
            }
    }
}

/// How a pane's request for somebody changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attend {
    /// This pane is asking for somebody and was not a moment ago.
    Raised(Alert),
    /// This pane has stopped asking, so anything already delivered about it is stale - it
    /// would land a person on a pane that no longer wants them.
    Withdrawn,
}

/// What looking at the window changed.
///
/// Three lists rather than one, because they answer different questions and overlap only by
/// coincidence. A finished pane looked at is a pane whose *state* is now `idle`, which the
/// border and the roster have to be told, and whose daemon has to be told too. A `blocked` pane
/// looked at has the same state it had a moment ago and a notification that has stopped being
/// true.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Noticed {
    /// Panes whose presented state changed, to be re-announced.
    pub settled: Vec<PaneKey>,
    /// Finished panes this look has seen, each to be reported to its daemon with `Seen`.
    pub reported: Vec<PaneKey>,
    /// Panes that have stopped asking for anybody.
    pub withdrawn: Vec<PaneKey>,
}

/// What one pane's record changed here.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Observed {
    pub attend: Option<Attend>,
    /// The pane finished in front of somebody, so its daemon is to be told it was seen.
    pub reported: bool,
}

/// What this window has seen, and what is still waiting for somebody.
#[derive(Debug, Default)]
pub struct Attention {
    /// Whether this window has the OS's focus. Starts false: a window that has not yet been
    /// told it is focused has not been seen through, and guessing the friendlier answer
    /// would mark agents seen that nobody looked at.
    focused: bool,

    /// The panes on screen right now. A pane hidden behind its tab's zoom is not among them,
    /// because the published tree holds only the zoomed pane when a tab is zoomed.
    visible: BTreeSet<PaneKey>,

    /// Panes whose daemon says their agent finished and nobody has seen it since: the
    /// daemon's `finished_unseen`, as last heard.
    finished: BTreeSet<PaneKey>,

    /// Finished panes this window has reported seen, and whose daemon has not yet said so
    /// back. Painted `idle` meanwhile, and let go of when the daemon clears the fact.
    reported: BTreeSet<PaneKey>,

    /// The panes asking for somebody right now, and what each is asking - the unread set
    /// `architecture.md` says the core owns.
    ///
    /// Deliberately not the same thing as `finished`, which decides what a pane *is*. A muted
    /// window still paints `done` on its borders and still lists it in the roster; what mute
    /// takes away is the interruption. Folding the two together would make a preference about
    /// banners silently change the state vocabulary this product is built on.
    raised: BTreeMap<PaneKey, Alert>,

    /// What each pane raised as [`Alert::Notified`] said.
    notes: BTreeMap<PaneKey, Note>,

    /// Panes whose program rang the bell since somebody last looked at them. A mark on the
    /// pane and never a banner: shells ring for a completion that found nothing, and a banner
    /// for each would teach somebody to mute everything.
    rang: BTreeSet<PaneKey>,

    notifications: Notifications,
}

impl Attention {
    pub fn new() -> Attention {
        Attention::default()
    }

    /// Takes a new answer about what is worth interrupting somebody for.
    ///
    /// Returns the panes whose delivered notifications the change has made stale. Nothing is
    /// raised retroactively when a state is switched back on: a notification is about the
    /// moment a pane started asking, and an agent that has been waiting ten minutes is
    /// already on its own border and in the roster. Answering a save with a banner for
    /// something already on screen would be the config file shouting about itself.
    pub fn notifying(&mut self, notifications: Notifications) -> Vec<PaneKey> {
        self.notifications = notifications;
        let stale: Vec<PaneKey> = self
            .raised
            .iter()
            .filter(|(_, alert)| !notifications.allows(**alert))
            .map(|(pane, _)| pane.clone())
            .collect();
        for pane in &stale {
            self.raised.remove(pane);
            self.notes.remove(pane);
        }
        stale
    }

    /// Every pane asking for somebody, the one being waited on first.
    ///
    /// The ordering is the urgency ordering rather than an incidental one: `blocked` is
    /// somebody held up right now and `done` is somebody who was held up at some point, so a
    /// reader working down this list works down it in the order that costs least.
    pub fn asking(&self) -> Vec<(&PaneKey, Alert)> {
        let mut asking: Vec<(&PaneKey, Alert)> =
            self.raised.iter().map(|(pane, alert)| (pane, *alert)).collect();
        asking.sort_by_key(|(pane, alert)| (*alert, *pane));
        asking
    }

    /// A pane this window is meeting for the first time, as its daemon already had it.
    ///
    /// Raises nothing, whatever it says. Muster witnessed no transition here, and quitting and
    /// coming back is the ordinary case, so a banner would be Muster announcing history at
    /// launch. A finish it carries is still `done` on the border and in the roster until a look
    /// finds it.
    ///
    /// Returns whether the window is already showing it to somebody, in which case it is
    /// reported seen: a daemon's panes are published before they are met.
    pub fn met(&mut self, pane: &PaneKey, finished: bool) -> bool {
        if !finished {
            return false;
        }
        self.finished.insert(pane.clone());
        self.seen(pane) && self.reported.insert(pane.clone())
    }

    /// Takes a pane's record after its agent moved or its finish was set or cleared: whether
    /// that changes what the pane is asking of the person, and whether it finished in front of
    /// somebody.
    ///
    /// `blocked` asks whenever the agent is in it, and a repeat of the same answer is not news.
    /// A finish asks once, when the daemon first says it, so a pane already `done` whose agent
    /// then quits does not ask again. Anything else takes a request back: an agent that went
    /// back to work, and a finish another window saw and cleared, are no longer waiting on
    /// anybody.
    ///
    /// A pane the window is focused on and showing raises nothing, in either state. That is
    /// what the border is for, and a banner about a pane somebody is looking at is the fastest
    /// way to teach them to turn banners off. A finish there is reported seen instead.
    pub fn observed(&mut self, pane: &PaneKey, state: AgentState, finished: bool) -> Observed {
        let newly_finished = finished && self.finished.insert(pane.clone());
        if !finished {
            self.finished.remove(pane);
            self.reported.remove(pane);
        }
        if self.seen(pane) {
            let reported = finished && self.reported.insert(pane.clone());
            return Observed { attend: self.withdraw(pane), reported };
        }
        let attend = if state == AgentState::Blocked {
            self.raise(pane, Alert::Blocked)
        } else if self.raised.get(pane) == Some(&Alert::Notified) {
            // A program's request stands until somebody looks, whatever its agent does next.
            None
        } else if newly_finished {
            self.raise(pane, Alert::Done)
        } else if finished && self.raised.get(pane) == Some(&Alert::Done) {
            None
        } else {
            self.withdraw(pane)
        };
        Observed { attend, reported: false }
    }

    /// A program in the pane rang the bell. Returns whether that marked the pane, which it does
    /// unless somebody is looking at it or it is marked already.
    pub fn bell(&mut self, pane: &PaneKey) -> bool {
        !self.seen(pane) && self.rang.insert(pane.clone())
    }

    /// Whether a bell in this pane has gone unseen.
    pub fn has_rung(&self, pane: &PaneKey) -> bool {
        self.rang.contains(pane)
    }

    /// A program in the pane asked to notify somebody. `agent` is the harness the pane's daemon
    /// recognized in it, if any.
    ///
    /// Asks unless somebody is looking at the pane, the file says programs are not worth
    /// interrupting for, or the pane is already blocked, which is the more urgent ask. Nor does
    /// it ask again while the pane is still asking with a notification of its own: a program that
    /// notifies in a loop would otherwise post a banner and a sound each time, and the first has
    /// already said the pane wants somebody. So the banner keeps the first one's words.
    ///
    /// Nor does a pane running an agent Muster recognizes. An agent notifies at the moments its
    /// state already asks about - Claude Code when it has sat idle, and at a permission prompt -
    /// so a banner of its own is `done` or `blocked` asked twice, and one that ignores what the
    /// file says about them. Its shell's programs ask again once it has gone.
    pub fn notified(&mut self, pane: &PaneKey, note: Note, agent: Option<&str>) -> Option<Attend> {
        if agent.is_some()
            || self.seen(pane)
            || !self.notifications.allows(Alert::Notified)
            || matches!(self.raised.get(pane), Some(Alert::Blocked | Alert::Notified))
        {
            return None;
        }
        self.notes.insert(pane.clone(), note);
        self.raised.insert(pane.clone(), Alert::Notified);
        Some(Attend::Raised(Alert::Notified))
    }

    /// What the pane's program said, while the pane is asking with it.
    pub fn note(&self, pane: &PaneKey) -> Option<&Note> {
        if self.raised.get(pane) == Some(&Alert::Notified) { self.notes.get(pane) } else { None }
    }

    /// What the window should show for a pane, given what its daemon says the agent is doing.
    ///
    /// A finish is `done` whether the agent is idle or has left the pane, which is how a crash
    /// or a one-shot run ends. The daemon clears it when the agent works or waits on somebody
    /// again, so a busy state should never arrive carrying one; if it does, the busy state wins.
    /// So does `waiting`, which the daemon sets no finish under and clears one for.
    pub fn presented(&self, pane: &PaneKey, state: AgentState) -> AgentState {
        let busy = matches!(state, AgentState::Working | AgentState::Blocked | AgentState::Waiting);
        if !busy && self.finished.contains(pane) && !self.reported.contains(pane) {
            AgentState::Done
        } else {
            state
        }
    }

    /// The window gained or lost the OS's focus.
    ///
    /// Returns the panes whose presentation this changed, so a caller can re-announce those
    /// and nothing else - an agent-state change costs that change rather than a walk of
    /// every pane (`architecture.md`, fast is a feature).
    ///
    /// Losing focus changes nothing, and that asymmetry is the point: looking away from a
    /// pane you already looked at does not un-see it.
    pub fn window_focused(&mut self, focused: bool) -> Noticed {
        self.focused = focused;
        if focused { self.noticed() } else { Noticed::default() }
    }

    /// What the window is showing now.
    ///
    /// Returns the panes whose presentation this changed, on the same terms as
    /// [`Attention::window_focused`].
    pub fn showing(&mut self, visible: BTreeSet<PaneKey>) -> Noticed {
        self.visible = visible;
        if self.focused { self.noticed() } else { Noticed::default() }
    }

    /// Lets go of a pane the backend no longer holds.
    ///
    /// Called when a pane closes or exits. A name can come back: a daemon that restarts brings
    /// each pane back under its old name with a new shell in it, and what is held here about
    /// the old one must not be inherited by the new.
    ///
    /// Says so when the pane was asking for somebody, because a notification outliving its
    /// pane is one that answers a click by focusing nothing.
    pub fn forget(&mut self, pane: &PaneKey) -> Option<Attend> {
        self.rang.remove(pane);
        self.finished.remove(pane);
        self.reported.remove(pane);
        self.visible.remove(pane);
        self.withdraw(pane)
    }

    /// A daemon's connection came back, so what this window reported to it may never have
    /// arrived.
    ///
    /// Its reports are taken back, and each of those panes is `done` again until a look finds
    /// it: a finish the daemon still holds may be one it never heard was seen, or a new one
    /// that arrived while the connection was down. Painting it `done` is the answer that can be
    /// wrong only by asking somebody to look. What is on screen in a focused window is reported
    /// again at once.
    pub fn reconnected(&mut self, daemon: &DaemonId) -> Noticed {
        let taken_back: Vec<PaneKey> =
            self.reported.iter().filter(|pane| &pane.daemon == daemon).cloned().collect();
        for pane in &taken_back {
            self.reported.remove(pane);
        }
        let mut noticed = if self.focused { self.noticed() } else { Noticed::default() };
        noticed.settled =
            taken_back.into_iter().filter(|pane| !noticed.reported.contains(pane)).collect();
        noticed
    }

    /// A daemon refused this window's report that it saw these panes, as one partway through
    /// handing its panes to another refuses every change.
    ///
    /// The reports are taken back, so each pane is `done` again, as the daemon and every other
    /// window still have it, and returns the ones whose presentation that changed. Nothing is
    /// reported again at once, since a daemon still refusing would refuse that too: the next
    /// look reports it, as [`Attention::reconnected`] does for a daemon that comes back.
    pub fn refused(&mut self, panes: &[PaneKey]) -> Vec<PaneKey> {
        panes.iter().filter(|pane| self.reported.remove(*pane)).cloned().collect()
    }

    /// Reports every finished pane now being looked at, and takes back what any on-screen
    /// pane was asking.
    fn noticed(&mut self) -> Noticed {
        let reported: Vec<PaneKey> = self
            .finished
            .intersection(&self.visible)
            .filter(|pane| !self.reported.contains(*pane))
            .cloned()
            .collect();
        self.reported.extend(reported.iter().cloned());
        let withdrawn: Vec<PaneKey> =
            self.raised.keys().filter(|pane| self.visible.contains(*pane)).cloned().collect();
        for pane in &withdrawn {
            self.raised.remove(pane);
            self.notes.remove(pane);
        }
        let heard: Vec<PaneKey> = self.rang.intersection(&self.visible).cloned().collect();
        for pane in &heard {
            self.rang.remove(pane);
        }
        // A pane that finished and rang is announced once, for both.
        let mut settled = reported.clone();
        settled.extend(heard.into_iter().filter(|pane| !reported.contains(pane)));
        Noticed { settled, reported, withdrawn }
    }

    /// Starts this pane asking, unless the file says that state is not worth interrupting for.
    ///
    /// A state that is muted withdraws rather than merely declining to raise: a pane that
    /// asked under the old setting is still on somebody's screen, and leaving it there would
    /// make a mute mean "no new ones" rather than "quiet".
    fn raise(&mut self, pane: &PaneKey, alert: Alert) -> Option<Attend> {
        if !self.notifications.allows(alert) {
            return self.withdraw(pane);
        }
        // An unchanged answer is not news. A pane that blocks, is re-reported as blocked, and
        // blocks again should interrupt somebody once.
        if self.raised.insert(pane.clone(), alert) == Some(alert) {
            return None;
        }
        Some(Attend::Raised(alert))
    }

    fn withdraw(&mut self, pane: &PaneKey) -> Option<Attend> {
        self.notes.remove(pane);
        self.raised.remove(pane).map(|_| Attend::Withdrawn)
    }

    fn seen(&self, pane: &PaneKey) -> bool {
        self.focused && self.visible.contains(pane)
    }
}
