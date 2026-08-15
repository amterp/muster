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
//! Pure - no clock, no window, no socket. Seen-ness is a fold over transitions and what the
//! window was showing at the time, which is exactly what a recorded case can drive.

use std::collections::BTreeSet;

use crate::AgentState;
use crate::composition::PaneKey;

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
    /// the whole of the state: everything else is derived from it and the daemon's own word.
    unseen: BTreeSet<PaneKey>,
}

impl Attention {
    pub fn new() -> Attention {
        Attention::default()
    }

    /// Records one transition, and whether it counts as finishing unseen.
    ///
    /// Mirrors herdr's own rule, which is worth matching rather than improving on: anything
    /// that is not idle means the agent is doing something, so the pane is no longer waiting
    /// on anyone. Only a *completion* - working or blocked falling to idle - can leave a
    /// pane unseen. An idle that arrives from anywhere else (a pane whose harness we could
    /// not read, or a repeated idle) changes nothing, because nothing finished.
    pub fn observed(&mut self, pane: &PaneKey, from: AgentState, to: AgentState) {
        let (from, to) = (settled(from), settled(to));
        if to != AgentState::Idle {
            self.unseen.remove(pane);
            return;
        }
        if !matches!(from, AgentState::Working | AgentState::Blocked) {
            return;
        }
        if self.seen(pane) {
            self.unseen.remove(pane);
        } else {
            self.unseen.insert(pane.clone());
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
    pub fn window_focused(&mut self, focused: bool) -> Vec<PaneKey> {
        self.focused = focused;
        if focused { self.mark_visible_seen() } else { Vec::new() }
    }

    /// What the window is showing now.
    ///
    /// Returns the panes whose presentation this changed, on the same terms as
    /// [`Attention::window_focused`].
    pub fn showing(&mut self, visible: BTreeSet<PaneKey>) -> Vec<PaneKey> {
        self.visible = visible;
        if self.focused { self.mark_visible_seen() } else { Vec::new() }
    }

    /// Drops every pane now being looked at out of the waiting set, and says which they were.
    fn mark_visible_seen(&mut self) -> Vec<PaneKey> {
        let seen: Vec<PaneKey> = self.unseen.intersection(&self.visible).cloned().collect();
        for pane in &seen {
            self.unseen.remove(pane);
        }
        seen
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
