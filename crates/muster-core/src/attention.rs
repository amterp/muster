//! Which agents have been seen, and which are still waiting to be.
//!
//! Four of the five agent states are daemon truth and arrive as themselves. `done` is not:
//! it is `idle` on a pane nobody has looked at, and whether anybody looked is a fact about
//! a window rather than about a session.
//!
//! herdr derives it too, from its own two inputs - whether the pane's tab is the daemon's
//! active tab, and whether the foreground client's host window has OS focus. Neither is
//! reachable over its public JSON API: `pane.mark_seen`, `client.focus` and
//! `client.outer_focus` are all unknown methods, and the DEC focus sequences go to the
//! pane's program rather than to the client-focus machinery
//! (`docs/observations/herdr-0.8.0.md` section 3). So a daemon asked to decide this for a
//! window it cannot see answers from a client that never reported, and a Muster window
//! sitting unfocused while an agent finishes gets `idle` - which reads as "nothing needs
//! you" at the exact moment something does.
//!
//! Muster holds both halves already. The agent channel delivers every transition, and the
//! shell is the only thing that knows whether its own window had focus when one arrived.
//! So `done` is computed here and herdr's own answer is normalized away on the way in.
//! Two writers for one field is the failure `architecture.md` warns about, and of the two
//! this one can actually see the window.
//!
//! What that costs, stated rather than hidden: ours is the only focus we can observe, so
//! `done` means "nobody *we know of* saw it". A second Muster window, or a herdr TUI open
//! beside us, is outside what this can answer.
//!
//! **And which of them are worth interrupting somebody for.** Glanceable states are the
//! floor rather than the ceiling: a pane no region is showing is exactly the pane most
//! likely to be waiting, and a border it is not drawing tells nobody. So this also holds
//! the set of panes currently asking for a person, and decides when one joins or leaves it.
//! That is the split `architecture.md` draws, where the core owns the unread set and the
//! shell only delivers: what a banner says and how it is posted is an OS question, and is
//! not here.
//!
//! Pure - no clock, no window, no socket. Seen-ness is a fold over transitions and what the
//! window was showing at the time, which is exactly what a recorded case can drive.

use std::collections::{BTreeMap, BTreeSet};

use crate::AgentState;
use crate::composition::PaneKey;

/// What a pane is asking of the person, when it is asking anything.
///
/// Two of the five states ask, and they ask the same question from opposite ends: `blocked`
/// is an agent that has stopped and wants an answer, and `done` is an agent that has stopped
/// and nobody has noticed. Neither `working` nor `idle` asks for anybody, and `unknown` is
/// the absence of an answer rather than one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Alert {
    /// An agent waiting on somebody. First, because it is the one somebody is holding up.
    Blocked,
    /// An agent that finished while nobody was looking.
    Done,
}

impl Alert {
    pub fn as_str(self) -> &'static str {
        match self {
            Alert::Blocked => "blocked",
            Alert::Done => "done",
        }
    }
}

/// Which of those are worth interrupting somebody for.
///
/// Both on by default, because both are a person being waited on and a state that never
/// notifies is a state you have to go and look for. The mute is the answer for fifteen
/// agents at once, and it is a third key rather than "set both to false" so that turning
/// the noise off for an afternoon does not cost you the two answers underneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notifications {
    pub blocked: bool,
    pub done: bool,
    pub muted: bool,
}

impl Default for Notifications {
    fn default() -> Notifications {
        Notifications { blocked: true, done: true, muted: false }
    }
}

impl Notifications {
    fn allows(self, alert: Alert) -> bool {
        !self.muted
            && match alert {
                Alert::Blocked => self.blocked,
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
/// Two sets rather than one, because they answer different questions and overlap only by
/// coincidence. A `done` pane looked at is a pane whose *state* is now `idle`, which the
/// border and the roster have to be told. A `blocked` pane looked at has the same state it
/// had a moment ago and a notification that has stopped being true.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Noticed {
    /// Panes whose presented state changed, to be re-announced.
    pub settled: Vec<PaneKey>,
    /// Panes that have stopped asking for anybody.
    pub withdrawn: Vec<PaneKey>,
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

    /// Panes that finished while nobody was looking, and are still waiting to be. This is
    /// the whole of what decides a `done`: everything about the state is derived from it and
    /// the daemon's own word.
    unseen: BTreeSet<PaneKey>,

    /// The panes asking for somebody right now, and what each is asking - the unread set
    /// `architecture.md` says the core owns.
    ///
    /// Deliberately not the same thing as `unseen`, which decides what a pane *is*. A muted
    /// window still paints `done` on its borders and still lists it in the roster; what mute
    /// takes away is the interruption. Folding the two together would make a preference about
    /// banners silently change the state vocabulary this product is built on.
    raised: BTreeMap<PaneKey, Alert>,

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

    /// Records one transition: whether it counts as finishing unseen, and whether it changes
    /// what this pane is asking of the person.
    ///
    /// Mirrors herdr's own rule, which is worth matching rather than improving on: anything
    /// that is not idle means the agent is doing something, so the pane is no longer waiting
    /// on anyone. Only a *completion* - working or blocked falling to idle - can leave a
    /// pane unseen. An idle that arrives from anywhere else (a pane whose harness we could
    /// not read, or a repeated idle) changes nothing, because nothing finished.
    ///
    /// `blocked` is the exception to "doing something means not waiting". It is the one busy
    /// state that is busy waiting on a person, so it raises where every other non-idle
    /// transition withdraws.
    ///
    /// A pane the window is focused on and showing raises nothing, in either state. That is
    /// what the border is for, and a banner about a pane somebody is looking at is the fastest
    /// way to teach them to turn banners off.
    pub fn observed(&mut self, pane: &PaneKey, from: AgentState, to: AgentState) -> Option<Attend> {
        let (from, to) = (settled(from), settled(to));
        if to != AgentState::Idle {
            self.unseen.remove(pane);
            return if to == AgentState::Blocked && !self.seen(pane) {
                self.raise(pane, Alert::Blocked)
            } else {
                self.withdraw(pane)
            };
        }
        if !matches!(from, AgentState::Working | AgentState::Blocked) {
            return None;
        }
        if self.seen(pane) {
            self.unseen.remove(pane);
            self.withdraw(pane)
        } else {
            self.unseen.insert(pane.clone());
            self.raise(pane, Alert::Done)
        }
    }

    /// A pane this window is meeting for the first time, as the backend already had it.
    ///
    /// The one moment a backend's own `done` is worth taking, and the reason is that we have
    /// nothing better. Muster witnessed no transition for a pane that finished before it
    /// attached, and a daemon outlives the app, so quitting and coming back is the ordinary
    /// case rather than a corner of one. The daemon does have evidence there: it knows the
    /// pane's tab was in the background. Refusing that because we cannot personally vouch for
    /// it would mean a window opened after a break reports that nothing needs anybody, at the
    /// one moment several things do.
    ///
    /// The same shape as the rule the mirror already follows for the field itself: structure
    /// sets agent state only for a pane it is seeing for the first time, and the agent channel
    /// owns it from then on. Here, first sight adopts and every observation after it is ours.
    pub fn first_seen(&mut self, pane: &PaneKey, backend: AgentState) {
        if backend == AgentState::Done && !self.seen(pane) {
            self.unseen.insert(pane.clone());
        }
    }

    /// What the window should show for a pane, given what the daemon says about it.
    ///
    /// The daemon's `done` is normalized away first. It is a guess about a client that never
    /// reported its focus, so keeping it would leave two answers to one question and no way
    /// to tell which was which.
    pub fn presented(&self, pane: &PaneKey, backend: AgentState) -> AgentState {
        let backend = settled(backend);
        if backend == AgentState::Idle && self.unseen.contains(pane) {
            AgentState::Done
        } else {
            backend
        }
    }

    /// The window gained or lost the OS's focus.
    ///
    /// Returns the panes whose presentation this changed, so a caller can re-announce those
    /// and nothing else - an agent-state change costs that change rather than a walk of
    /// every pane (`architecture.md`, fast is a feature).
    ///
    /// Losing focus changes nothing, and that asymmetry is the point. Seen-ness is written
    /// when an agent finishes and never taken back: looking away from a pane you already
    /// looked at does not un-see it.
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
    /// Called when a pane closes or exits, and it matters for more than the bookkeeping. Ids
    /// are the backend's and the backend reuses them - a restarted herdr hands out `w1:p1`
    /// again - so an entry left behind is not merely dead weight: the next pane to be given
    /// that id inherits it, and a brand-new agent renders as `done` before it has done
    /// anything. Waiting to be looked at is the one piece of state here, and a pane that is
    /// gone is not waiting for anybody.
    ///
    /// Says so when the pane was asking for somebody, because a notification outliving its
    /// pane is one that answers a click by focusing nothing.
    pub fn forget(&mut self, pane: &PaneKey) -> Option<Attend> {
        self.unseen.remove(pane);
        self.visible.remove(pane);
        self.withdraw(pane)
    }

    /// Drops every pane now being looked at out of both sets, and says which they were.
    fn noticed(&mut self) -> Noticed {
        let settled: Vec<PaneKey> = self.unseen.intersection(&self.visible).cloned().collect();
        for pane in &settled {
            self.unseen.remove(pane);
        }
        let withdrawn: Vec<PaneKey> =
            self.raised.keys().filter(|pane| self.visible.contains(*pane)).cloned().collect();
        for pane in &withdrawn {
            self.raised.remove(pane);
        }
        Noticed { settled, withdrawn }
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
        self.raised.remove(pane).map(|_| Attend::Withdrawn)
    }

    fn seen(&self, pane: &PaneKey) -> bool {
        self.focused && self.visible.contains(pane)
    }
}

/// A backend's word about a pane, with its guess at seen-ness taken back out.
///
/// `done` is the only state a backend derives rather than observes, so it is the only one
/// Muster refuses to take at face value. Everything else is what the daemon saw.
fn settled(state: AgentState) -> AgentState {
    if state == AgentState::Done { AgentState::Idle } else { state }
}
