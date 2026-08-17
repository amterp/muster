//! A convergent picture of what a backend holds.
//!
//! The core owns a mirror: a derived, disposable cache of daemon structure, bootstrapped
//! from an authoritative snapshot plus an event subscription, rebuilt after any gap, never
//! patched across one (`docs/architecture.md`, ownership of truth).
//!
//! Pure by construction - no sockets, no threads, no clock. That is not tidiness: it is
//! what lets the whole of this behavior be judged by recorded cases rather than by staging
//! a daemon into each state (`docs/testing.md`).

use std::collections::BTreeMap;

use crate::mirror::ordered::Ordered;

use crate::AgentState;
use crate::intent::SettledLayout;
use crate::mirror::backend::{
    Focus, Health, Layout, LayoutNode, Pane, PaneId, Snapshot, Tab, TabId, Workspace, WorkspaceId,
};
use crate::mirror::event::{BackendEvent, Change};

/// How many arrangements one tab may be remembered as having moved past.
///
/// A leak bound rather than a tuning knob. What fills it is answers that have outrun their own
/// broadcast, so how many there can be is decided by two measured numbers: how fast the fastest
/// thing that moves a divider goes, and how far behind the daemon's broadcast is. Overflowing
/// drops the oldest, and the oldest is the one whose broadcast is about to arrive - so the cost
/// is the divider jumping back to where the gesture began.
///
/// Sized for a dragged divider, which is the fastest of them at about a hundred requests a
/// second against a broadcast a hundred milliseconds behind (`observations/herdr-0.8.0.md`
/// section 14, and kan a_28h3eBJa2) - so roughly ten arrangements are in flight at the worst
/// moment. Four was sized for a held resize chord at key-repeat speed and is three times too
/// small for a drag, which is what made a drag land back at its first position. The headroom
/// above ten is deliberate and nearly free: an entry is one tree, and being too small is
/// visible on screen while being too large costs a few hundred bytes on a tab nobody is
/// dragging.
const SUPERSEDED_LIMIT: usize = 32;

/// What the backend says is true, as far as this mirror knows.
///
/// Maps rather than vectors, ordered rather than hashed: iteration order is part of what
/// the log and the corpus compare, and a picture that reorders itself between runs is one
/// nobody can diff.
#[derive(Debug, Default)]
pub struct Mirror {
    workspaces: Ordered<WorkspaceId, Workspace>,
    tabs: Ordered<TabId, Tab>,
    panes: Ordered<PaneId, Pane>,
    /// One tree per tab that has a readable one. Absent rather than empty when a tab's
    /// layout will not read, because a tab with no tree renders as nothing and a tab
    /// keeping its last tree renders as slightly stale - and the second is the better
    /// wrong answer.
    layouts: BTreeMap<TabId, Layout>,
    /// Arrangements a tab has already moved past, still in flight on the subscription.
    ///
    /// A daemon answers a request faster than it broadcasts what the request did, so a mirror
    /// that takes the answer is ahead of its own event stream for a moment, and the events
    /// that arrive in that moment describe arrangements it has left behind. Rendering one is
    /// the pane jumping back to where it was, which is the whole defect this exists to
    /// prevent.
    ///
    /// The arrangement rather than the whole layout, because the arrangement is what flashes.
    /// The cursors beside it move on their own terms and disagreeing about them is normal:
    /// herdr's swap puts daemon focus on the source pane, so the tree published between the
    /// halves of a leftward split names a different focused pane than the one the tab settles
    /// on, while being exactly the arrangement that must not be drawn.
    ///
    /// Drained on the first match, so each entry costs at most one dropped event and nothing
    /// is suppressed indefinitely. Nothing clears an entry for *failing* to match, so an
    /// answer's broadcast arriving before the one it superseded is no worse than the other
    /// order.
    superseded: BTreeMap<TabId, Vec<LayoutNode>>,
    /// The arrangement an answer put here, until the backend's own broadcast of it arrives.
    ///
    /// What separates an arrangement still in flight from one long since delivered, and the
    /// reason [`Mirror::settle`] can tell whether the tree it is replacing is owed an echo.
    /// Without it, the tab's previous arrangement was armed every time - and a tab that later
    /// returns to that shape by closing a pane has its broadcast dropped, leaving a pane on
    /// screen that the daemon no longer holds.
    awaiting_echo: BTreeMap<TabId, LayoutNode>,
    /// Names given to panes the backend has not described yet.
    ///
    /// The other half of the rule below that a name is never taken from an event: the reply to
    /// `pane.rename` is the only statement of a rename there will ever be, and a pane Muster has
    /// just made spends a moment named on the backend and unknown here - the split's answer comes
    /// back before the pane's own event. A name applied in that moment lands on nothing, and
    /// because nothing announces one it is then gone for good: the pane appears under its
    /// directory, and every window shows an agent nobody named.
    ///
    /// Drained when the pane appears, cleared by a snapshot that describes it - a snapshot is
    /// taken after the answer and carries the backend's own label, so it is the better authority -
    /// and dropped with the pane if it turns out never to have survived. Nothing forgets one on a
    /// timer, because "the daemon is slow today" is exactly the case this exists for.
    names_awaiting_pane: BTreeMap<PaneId, Option<String>>,
    focus: Focus,
    health: Health,
    /// Why the health is what it is, for anyone who has to say so out loud. Empty when
    /// connected, because a live connection needs no excuse.
    ///
    /// Kept beside the health rather than only logged, because it is asked for outside the
    /// moment it happened: an event carries it to a shell that was listening, and a `Window`
    /// read has to answer a caller that was not. "stale" alone tells somebody their picture
    /// might be wrong and nothing about whether to wait or go looking.
    health_detail: String,
    /// The agent-state stamp the last snapshot carried. herdr issues these from one
    /// session-wide counter, so comparing two snapshots says how many transitions ran in
    /// between - including on panes this mirror has never heard of
    /// (`observations/herdr-0.8.0.md` section 10).
    agent_state_seq: Option<u64>,
    /// Agent transitions applied since that snapshot. The backend's counter minus this is
    /// how many ran without this mirror hearing about them.
    agent_transitions_applied: u64,
}

impl Mirror {
    pub fn new() -> Mirror {
        Mirror::default()
    }

    /// Replaces everything with what the backend just said, and reports what moved.
    ///
    /// Used for the first connection and for every reconnection, because herdr offers no
    /// replay: patching across a gap is the one thing convergence cannot save. Reporting
    /// the difference rather than "everything changed" is what lets a reconnect update the
    /// view without repainting panes that were never affected.
    pub fn bootstrap(&mut self, snapshot: Snapshot) -> Vec<Change> {
        let previous_panes = std::mem::take(&mut self.panes);
        let previous_tabs = std::mem::take(&mut self.tabs);
        let previous_workspaces = std::mem::take(&mut self.workspaces);
        let previous_layouts = std::mem::take(&mut self.layouts);
        let previous_focus = std::mem::replace(&mut self.focus, snapshot.focus);

        self.workspaces = snapshot.workspaces.into_iter().map(|w| (w.id.clone(), w)).collect();
        self.tabs = snapshot.tabs.into_iter().map(|t| (t.id.clone(), t)).collect();
        self.panes = snapshot.panes.into_iter().map(|p| (p.id.clone(), p)).collect();
        self.layouts = snapshot.layouts.into_iter().map(|l| (l.tab.clone(), l)).collect();
        // A snapshot is a fresh statement of the whole world, taken after every answer this
        // was suppressing on behalf of. Keeping one across it would drop a real arrangement
        // out of the stream that follows.
        self.superseded.clear();
        self.awaiting_echo.clear();
        // Same reasoning, one step further: a snapshot describing the pane carries the backend's
        // own label for it, which is what the rename produced. Holding the wish past that would
        // let it put back a name a later client had already changed.
        self.names_awaiting_pane.retain(|pane, _| !self.panes.contains_key(pane));
        self.health = Health::Connected;
        self.health_detail.clear();
        let previous_seq = std::mem::replace(&mut self.agent_state_seq, snapshot.agent_state_seq);
        let applied = std::mem::take(&mut self.agent_transitions_applied);

        let mut changes = Vec::new();
        // Before the per-entity diff, because it is about the interval rather than about
        // any one pane: a transition that ran and reverted inside the gap leaves both
        // snapshots identical and shows up here or nowhere.
        if let (Some(previous), Some(saw)) = (previous_seq, snapshot.agent_state_seq) {
            let expected = previous.saturating_add(applied);
            if saw > expected {
                changes.push(Change::AgentTransitionsMissed { expected, saw });
            }
        }
        for id in self.workspaces.keys() {
            if !previous_workspaces.contains_key(id) {
                changes.push(Change::WorkspaceAdded(id.clone()));
            }
        }
        for id in previous_workspaces.keys() {
            if !self.workspaces.contains_key(id) {
                changes.push(Change::WorkspaceRemoved(id.clone()));
            }
        }
        for id in self.tabs.keys() {
            if !previous_tabs.contains_key(id) {
                changes.push(Change::TabAdded(id.clone()));
            }
        }
        for id in previous_tabs.keys() {
            if !self.tabs.contains_key(id) {
                changes.push(Change::TabRemoved(id.clone()));
            }
        }
        for (id, pane) in self.panes.iter() {
            match previous_panes.get(id) {
                None => changes.push(Change::PaneAdded(id.clone())),
                Some(before) if before.agent_state != pane.agent_state => {
                    changes.push(Change::AgentStateChanged {
                        pane: id.clone(),
                        from: before.agent_state,
                        to: pane.agent_state,
                    });
                }
                Some(_) => {}
            }
        }
        for id in previous_panes.keys() {
            if !self.panes.contains_key(id) {
                // Not cascaded: a snapshot is a fresh statement of the whole world, so
                // nothing here is an inference about what a parent took with it.
                changes.push(Change::PaneRemoved { pane: id.clone(), cascaded: false });
            }
        }
        // After the panes, because a tree names them: a reader told the arrangement first
        // would be handed a tree referring to a pane it has not been told exists.
        //
        // Reported for a tab whose tree moved, and for one that still exists and lost its
        // tree. A tab that went away is a TabRemoved and needs no second announcement about
        // the tree that went with it.
        for (tab, layout) in &self.layouts {
            if previous_layouts.get(tab) != Some(layout) {
                changes.push(Change::LayoutChanged(tab.clone()));
            }
        }
        for tab in previous_layouts.keys() {
            if !self.layouts.contains_key(tab) && self.tabs.contains_key(tab) {
                changes.push(Change::LayoutChanged(tab.clone()));
            }
        }
        if self.focus != previous_focus {
            changes.push(Change::FocusChanged);
        }
        changes
    }

    /// Applies one event, and reports what it actually changed.
    ///
    /// An event that changed nothing returns nothing. That is what makes idempotence
    /// observable rather than merely intended: the subscription replays the whole session
    /// as creation events, and a mirror that reported those as changes would repaint every
    /// pane on every reconnect.
    pub fn apply(&mut self, event: BackendEvent) -> Vec<Change> {
        // Focus is compared once, here, rather than reported by whichever branch touched
        // it. Three different paths move a cursor - an explicit focus event, a removal
        // that orphans one, a cascade two levels down - and each reporting for itself is
        // how one of them ends up not reporting at all.
        let focus_before = self.focus.clone();
        let mut changes = self.apply_inner(event);
        if self.focus != focus_before && !changes.contains(&Change::FocusChanged) {
            changes.push(Change::FocusChanged);
        }
        changes
    }

    /// Takes an arrangement the daemon stated in an answer, and remembers what it left behind.
    ///
    /// Distinct from applying an event, and the difference is only which channel the daemon
    /// said it on. A backend answers a request with the arrangement that request produced, and
    /// broadcasts the same arrangement afterwards - herdr about a hundred milliseconds
    /// afterwards, measured (`observations/herdr-0.8.0.md` section 14). Both are the daemon
    /// speaking, so both may be applied; taking the earlier one is the difference between a
    /// split that lands and a split that lands twice.
    ///
    /// What it leaves behind is what makes this safe. Between the answer and its broadcast the
    /// mirror is ahead of its own stream, and every arrangement the tab has passed through in
    /// that window is still queued up to arrive - the one it held before, always, and for a
    /// compound intent whatever the daemon published between the halves, which only the
    /// adapter knows. Both are armed here so that [`Mirror::apply`] can recognize them as news
    /// that has already been heard.
    ///
    /// An answer that changes nothing arms nothing: the tab is where the daemon says it should
    /// be, so there is no earlier arrangement still on its way.
    /// Takes what a backend said a pane is now called, from its answer to a rename.
    ///
    /// Daemon truth on the same terms as an event, like [`Mirror::settle`] beside it - and
    /// here it is the *only* terms there are. A backend need not announce a rename and herdr
    /// does not, so without this a pane named from this window changes on the daemon and not
    /// on screen, and stays that way until the connection re-snapshots.
    ///
    /// A rename of a pane the mirror does not hold is dropped rather than invented: it named
    /// a pane that has closed since, and a name is not enough to build one from.
    pub fn rename(&mut self, pane: &PaneId, name: Option<String>) -> Vec<Change> {
        match self.panes.get_mut(pane) {
            Some(held) if held.name != name => {
                held.name = name;
                vec![Change::PaneRelabelled(pane.clone())]
            }
            Some(_) => Vec::new(),
            // Kept rather than dropped, and nothing has changed on screen yet, so no change is
            // reported. See [`Mirror::names_awaiting_pane`] for why this moment exists at all.
            None => {
                self.names_awaiting_pane.insert(pane.clone(), name);
                Vec::new()
            }
        }
    }

    pub fn settle(&mut self, settled: SettledLayout) -> Vec<Change> {
        let SettledLayout { layout, stale } = settled;
        let tab = layout.tab.clone();
        let held = self.layouts.get(&tab);
        if held == Some(&layout) {
            return Vec::new();
        }

        // The tree being replaced, but only while the backend still owes a broadcast of it -
        // which is what happens under a chord going faster than the daemon can announce.
        // An arrangement the mirror reached by being *told* has already had its broadcast, and
        // arming it means dropping the next one that legitimately has that shape: closing a
        // pane collapses a tab back to a tree it held before, and that is the one broadcast
        // saying the pane is gone.
        let replaced = held
            .map(|held| held.root.clone())
            .filter(|root| self.awaiting_echo.get(&tab) == Some(root));

        let armed = self.superseded.entry(tab.clone()).or_default();
        // Never the arrangement being settled on. A tab whose answer only moved a cursor has
        // the same tree on both sides of this, and arming it would suppress the broadcast the
        // answer is waiting for rather than the one it overtook.
        armed.extend(
            replaced
                .into_iter()
                .chain(stale.map(|stale| stale.root))
                .filter(|passed| *passed != layout.root),
        );
        while armed.len() > SUPERSEDED_LIMIT {
            armed.remove(0);
        }

        self.awaiting_echo.insert(tab.clone(), layout.root.clone());
        self.layouts.insert(tab.clone(), layout);
        vec![Change::LayoutChanged(tab)]
    }

    /// Whether this arrangement is one its tab has already been told it left.
    ///
    /// Drains the entry it matched, so the same arrangement arriving twice is suppressed once.
    fn already_moved_past(&mut self, layout: &Layout) -> bool {
        let Some(armed) = self.superseded.get_mut(&layout.tab) else { return false };
        let Some(at) = armed.iter().position(|passed| *passed == layout.root) else {
            return false;
        };
        armed.remove(at);
        if armed.is_empty() {
            self.superseded.remove(&layout.tab);
        }
        true
    }

    /// Takes what a backend says a workspace is, and says whether anybody has to be told.
    ///
    /// **A label that moved is reported, because one is drawn.** Nothing draws a workspace on
    /// its own, which is why this used to report nothing - but a tab caption leads with the
    /// workspace holding it whenever a daemon holds more than one
    /// ([`crate::roster::Roster`]), so a workspace renamed restyles a row per tab. Silent, the
    /// mirror stored the new label and every caption went on reading the old one until some
    /// unrelated event forced a republish.
    ///
    /// **A replay carrying the label already held is silent**, and that is what keeps the
    /// above cheap: a subscription replays the whole session on every reconnect, so reporting
    /// every upsert would republish once per workspace for nothing.
    ///
    /// Only the telling changed. The label was written either way, so this is not the mirror
    /// storing something new - it is the mirror no longer keeping it to itself.
    fn upsert_workspace(&mut self, workspace: Workspace) -> Vec<Change> {
        let id = workspace.id.clone();
        let renamed =
            self.workspaces.get(&id).is_some_and(|before| before.label != workspace.label);
        match self.workspaces.insert(id.clone(), workspace) {
            Some(_) if renamed => vec![Change::WorkspaceRelabelled(id)],
            Some(_) => Vec::new(),
            None => vec![Change::WorkspaceAdded(id)],
        }
    }

    /// Puts one workspace's tabs in the order the backend says they now sit.
    ///
    /// Adopted rather than checked against the tabs held: an order naming one this has never
    /// heard of is not an order to refuse, because the tab's own creation is on its way and
    /// every other tab would sit where it was for a reason nobody could see.
    ///
    /// Silent when nothing moved, so a re-stated order costs a comparison rather than a
    /// republish of the whole window. A subscription replays a session's orders on reconnect,
    /// which makes that the common case rather than the rare one.
    fn reorder_tabs(&mut self, workspace: WorkspaceId, order: &[TabId]) -> Vec<Change> {
        if self.tabs.reorder(order) { vec![Change::TabsReordered(workspace)] } else { Vec::new() }
    }

    fn apply_inner(&mut self, event: BackendEvent) -> Vec<Change> {
        match event {
            BackendEvent::WorkspaceUpserted(workspace) => self.upsert_workspace(workspace),
            BackendEvent::WorkspaceRemoved(id) => self.remove_workspace(&id),
            // A tab that already exists keeps the name it already has. This event says a tab
            // exists, and a backend may replay it forever - herdr's carries the label the tab
            // was made with, which is its position, so applying one to a renamed tab puts a
            // number back over somebody's name. What the caption then does makes it worse
            // rather than obvious: it drops an all-digits label to suppress that very number,
            // so the row goes blank instead of wrong. Renaming has its own event below.
            BackendEvent::TabUpserted(mut tab) => {
                let id = tab.id.clone();
                if let Some(before) = self.tabs.get(&id) {
                    tab.label.clone_from(&before.label);
                    self.tabs.insert(id, tab);
                    return Vec::new();
                }
                self.tabs.insert(id.clone(), tab);
                vec![Change::TabAdded(id)]
            }
            // Only for a tab already held: a rename of something this mirror has never heard
            // of is not a tab it can invent, since the event carries a name and nothing else.
            // The creation it missed will arrive, or the next snapshot will.
            BackendEvent::TabRenamed { tab, label } => match self.tabs.get_mut(&tab) {
                Some(held) if held.label != label => {
                    held.label = label;
                    vec![Change::TabRelabelled(tab)]
                }
                _ => Vec::new(),
            },
            BackendEvent::TabRemoved(id) => self.remove_tab(&id),
            BackendEvent::TabsReordered { workspace, order } => {
                self.reorder_tabs(workspace, &order)
            }
            // Structure only, on a pane that already exists. A backend that carries agent
            // state on its structure events is a second writer for it, and the older of
            // the two: herdr replays the session as it stood when the subscription opened,
            // so a replayed pane would roll a live agent state back to whatever it was
            // then, with nothing arriving afterwards to correct it - agent state comes on
            // its own per-pane subscription. That channel owns the field, and a pane's
            // first appearance is the only thing taken from here.
            BackendEvent::PaneUpserted(mut pane) => {
                let id = pane.id.clone();
                let Some(before) = self.panes.get(&id) else {
                    // A name this pane was already given, before the backend got round to
                    // mentioning it. Taken now or lost: the event carries whatever label the
                    // backend held when it built the payload, which for a pane being introduced
                    // is usually none.
                    if let Some(named) = self.names_awaiting_pane.remove(&id) {
                        pane.name = named;
                    }
                    self.panes.insert(id.clone(), pane);
                    return vec![Change::PaneAdded(id)];
                };
                pane.agent_state = before.agent_state;
                pane.agent = pane.agent.or_else(|| before.agent.clone());
                // Two more fields structure events may not simply write, for the same reason
                // as agent state above and measured rather than assumed. herdr's replay is a
                // ring buffer of past events rather than a fresh statement of the world, and
                // a subscription drains it *after* its snapshot - so a reconnect replays the
                // pane's creation payload, which carries neither field because neither
                // existed yet, and then replays whatever came after it carrying what they
                // used to say. Applying either on top of the snapshot is going backwards.
                //
                // **A name is never taken from an event on a pane already held.** herdr
                // announces a rename to nobody and stamps no counter for one, so a `label`
                // on an event is not news: it is whatever was true when that event was
                // built, and there is nothing to order it by. A snapshot is the only
                // authority, which costs exactly one thing - a rename made by another client
                // reaches this window on the next re-snapshot rather than at once. That was
                // nearly true anyway, since nothing announces one; what is bought is that a
                // reconnect can no longer put back a name the session has moved past. Found
                // in the running app, with every layer under it green.
                //
                // **A title is taken when it is not older than what is held**, because that
                // one herdr does count: `revision` moves on a changed stripped title and on
                // nothing else, so it orders exactly this field. An absent title still means
                // "this payload does not speak to it" rather than "cleared", or a reconnect
                // would wipe every one in the window - and the cost of that direction is a
                // title its program genuinely clears staying until the next change.
                pane.name.clone_from(&before.name);
                if pane.revision >= before.revision {
                    pane.title = pane.title.or_else(|| before.title.clone());
                } else {
                    pane.title.clone_from(&before.title);
                    pane.revision = before.revision;
                }
                // What the pane is called, which is not structure and is not state. A pane
                // that changed directory is listed under a name it no longer has until
                // something else happens to move, and on a quiet session that is never.
                let relabelled = pane.cwd != before.cwd
                    || pane.agent != before.agent
                    || pane.name != before.name
                    || pane.title != before.title;
                self.panes.insert(id.clone(), pane);
                if relabelled { vec![Change::PaneRelabelled(id)] } else { Vec::new() }
            }
            BackendEvent::PaneRemoved(id) => self.remove_pane(&id, false),
            // Kept even for a tab this mirror does not know. herdr sends the layout for a
            // tab it has just created, and nothing says the tab event arrives first - a
            // tree dropped for arriving early would be replaced by nothing until the next
            // pane change, which on a quiet tab is never.
            BackendEvent::LayoutUpserted(layout) => {
                // Before the comparison below rather than after it, because the two say
                // different things: this one is an arrangement the daemon has already told
                // Muster it left, arriving late, and applying it would walk a tab backwards.
                if self.already_moved_past(&layout) {
                    return Vec::new();
                }
                let tab = layout.tab.clone();
                // The echo of an answer this mirror already took. Applying it is a no-op by
                // the comparison below; what matters is that the debt is settled, so the tree
                // stops counting as one the backend still owes a broadcast of.
                if self.awaiting_echo.get(&tab) == Some(&layout.root) {
                    self.awaiting_echo.remove(&tab);
                }
                if self.layouts.get(&tab) == Some(&layout) {
                    return Vec::new();
                }
                self.layouts.insert(tab.clone(), layout);
                vec![Change::LayoutChanged(tab)]
            }
            BackendEvent::AgentStateChanged { pane, state } => {
                // Counted whether or not it moved anything, because the backend's counter
                // is what this is reconciled against and the backend counted it.
                self.agent_transitions_applied += 1;
                let Some(existing) = self.panes.get_mut(&pane) else {
                    // A state change for a pane we do not know is dropped rather than
                    // inventing one: an agent event carries no tab or workspace, so a pane
                    // built from it would be an orphan the view could not place.
                    return Vec::new();
                };
                let from = existing.agent_state;
                if from == state {
                    return Vec::new();
                }
                existing.agent_state = state;
                vec![Change::AgentStateChanged { pane, from, to: state }]
            }
            BackendEvent::AgentDetected { pane, agent } => {
                if let Some(existing) = self.panes.get_mut(&pane)
                    && existing.agent.as_deref() != Some(agent.as_str())
                {
                    existing.agent = Some(agent);
                    // herdr detects a harness by reading the screen, seconds after the pane
                    // started, so this is how a list learns which of its rows is the one
                    // running claude. Reported now that something names panes by it.
                    return vec![Change::PaneRelabelled(pane)];
                }
                Vec::new()
            }
            // Each cursor is set only when this event names it. `None` means the event
            // says nothing about that cursor, not that it was cleared - herdr moves the
            // three with separate events, so treating absence as a clear would make a
            // pane focus silently unfocus the workspace. Whether anything moved is
            // decided by the caller, which compares the whole cursor set.
            BackendEvent::FocusMoved { workspace, tab, pane } => {
                if workspace.is_some() {
                    self.focus.workspace = workspace;
                }
                if tab.is_some() {
                    self.focus.tab = tab;
                }
                if pane.is_some() {
                    self.focus.pane = pane;
                }
                Vec::new()
            }
        }
    }

    /// Removing a workspace takes its tabs, and their panes, with it.
    ///
    /// The backend announces only the workspace: of five panes in the recorded lifecycle
    /// capture, two disappeared with a parent and got no pane event of any kind
    /// (`observations/herdr-0.8.0.md` section 10). A mirror that removed only what it was
    /// told about would keep rendering them, and they would be panes the user has no way
    /// to close.
    fn remove_workspace(&mut self, id: &WorkspaceId) -> Vec<Change> {
        if self.workspaces.remove(id).is_none() {
            return Vec::new();
        }
        let orphaned: Vec<TabId> = self
            .tabs
            .values()
            .filter(|tab| &tab.workspace == id)
            .map(|tab| tab.id.clone())
            .collect();
        let mut changes = Vec::new();
        for tab in orphaned {
            changes.extend(self.remove_tab(&tab));
        }
        changes.push(Change::WorkspaceRemoved(id.clone()));
        self.forget_focus();
        changes
    }

    /// Only panes carry whether their removal was announced or inferred, because only
    /// panes are rendered - a tab disappearing with its workspace has no surface to
    /// explain, while a pane does.
    fn remove_tab(&mut self, id: &TabId) -> Vec<Change> {
        if self.tabs.remove(id).is_none() {
            return Vec::new();
        }
        // The tree goes with the tab and is not reported separately: `layout_updated` does
        // not fire for a tab closing, so a mirror that waited for one would keep a tree for
        // a tab nobody can reach (`observations/herdr-0.8.0.md` section 10).
        self.layouts.remove(id);
        self.superseded.remove(id);
        self.awaiting_echo.remove(id);
        let orphaned: Vec<PaneId> = self
            .panes
            .values()
            .filter(|pane| &pane.tab == id)
            .map(|pane| pane.id.clone())
            .collect();
        let mut changes = Vec::new();
        for pane in orphaned {
            changes.extend(self.remove_pane(&pane, true));
        }
        changes.push(Change::TabRemoved(id.clone()));
        self.forget_focus();
        changes
    }

    fn remove_pane(&mut self, id: &PaneId, cascaded: bool) -> Vec<Change> {
        // Before the early return, because the pane this is about may be one that was named and
        // then died before the backend ever described it - and then this is the only word of it.
        self.names_awaiting_pane.remove(id);
        let Some(gone) = self.panes.remove(id) else {
            // A removal for something already gone is a no-op rather than an error. The
            // subscription replays, reconnects re-snapshot, and both routinely say a
            // thing is gone that this mirror already dropped.
            return Vec::new();
        };
        self.forget_focus();
        let mut changes = vec![Change::PaneRemoved { pane: id.clone(), cascaded }];

        // A tab whose last pane went is a tab the backend has already closed, and it says
        // nothing about it: closing or exiting the only pane in a tab emits `pane_closed` or
        // `pane_exited` and never a `tab_closed`, while `tab.list` stops reporting the tab
        // from that moment (`observations/herdr-0.8.0.md` section 15). So the mirror infers
        // it, or holds a tab nothing can reach forever - which is what a sidebar caption over
        // no rows looks like, and what keeps a region pointed at a tab that is gone.
        //
        // Safe to infer because a tab is never legitimately empty here: on creation the pane
        // arrives *before* its tab, measured on the same events, so there is no moment where
        // a real tab is waiting for its first pane.
        //
        // Not when cascading, because then the tab is what is being removed and this is one
        // of its orphans - inferring there would recurse into the removal already in flight.
        if !cascaded && !self.panes.values().any(|pane| pane.tab == gone.tab) {
            changes.extend(self.remove_tab(&gone.tab));
        }
        changes
    }

    /// Drops focus cursors pointing at things that no longer exist.
    ///
    /// A cursor naming a removed pane is worse than an empty one: every reader has to
    /// handle the dangling case, and the one that forgets renders a pane that is gone.
    fn forget_focus(&mut self) {
        if self.focus.pane.as_ref().is_some_and(|id| !self.panes.contains_key(id)) {
            self.focus.pane = None;
        }
        if self.focus.tab.as_ref().is_some_and(|id| !self.tabs.contains_key(id)) {
            self.focus.tab = None;
        }
        if self.focus.workspace.as_ref().is_some_and(|id| !self.workspaces.contains_key(id)) {
            self.focus.workspace = None;
        }
    }

    pub fn mark_stale(&mut self, detail: &str) {
        self.health = Health::Stale;
        self.health_detail = detail.to_string();
    }

    pub fn mark_disconnected(&mut self, detail: &str) {
        self.health = Health::Disconnected;
        self.health_detail = detail.to_string();
    }

    pub fn health(&self) -> Health {
        self.health
    }

    pub fn health_detail(&self) -> &str {
        &self.health_detail
    }

    pub fn pane(&self, id: &PaneId) -> Option<&Pane> {
        self.panes.get(id)
    }

    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
    }

    pub fn tab(&self, id: &TabId) -> Option<&Tab> {
        self.tabs.get(id)
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.values()
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values()
    }

    pub fn panes_in_tab<'a>(&'a self, tab: &'a TabId) -> impl Iterator<Item = &'a Pane> {
        self.panes.values().filter(move |pane| &pane.tab == tab)
    }

    /// How a tab arranges its panes, if the backend has said.
    ///
    /// `None` is a real answer rather than an empty one: a tab whose layout has not
    /// arrived, or would not read, is a tab a view should leave as it is.
    pub fn layout(&self, tab: &TabId) -> Option<&Layout> {
        self.layouts.get(tab)
    }

    pub fn layouts(&self) -> impl Iterator<Item = &Layout> {
        self.layouts.values()
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    pub fn agent_state(&self, id: &PaneId) -> Option<AgentState> {
        self.panes.get(id).map(|pane| pane.agent_state)
    }

    /// Takes the daemon's current answer for one pane's agent, having asked for it.
    ///
    /// Distinct from applying an event, and the difference is where the answer came from. A
    /// backend delivers a transition it witnessed; this is a question Muster asked because it
    /// knows it may have missed one - herdr delivers agent state only to a subscriber that
    /// names the pane, so between a pane existing and its subscription being live there is a
    /// window, and herdr offers no replay for what fell in it. Without this, a pane whose
    /// agent moved in that window keeps its old state and looks calm, which is the founding
    /// desideratum failing silently at the moment it matters most: just after a split, with
    /// something new started in the pane.
    ///
    /// **Not counted as a transition**, because the daemon counted one and Muster never saw
    /// it. `agent_transitions_applied` is reconciled against the backend's own counter to
    /// notice gaps, so counting a recovery would hide the very gap it recovered from - the
    /// next bootstrap would find the numbers agreeing and report nothing missed.
    ///
    /// `expected` is what the caller believed before it asked, and the answer is refused if
    /// the mirror has moved since. That is the ordering rule: a live subscription is a better
    /// authority than an answer to a question asked at the same moment, so anything that
    /// arrived while the question was in flight wins. Passing the state read a moment earlier
    /// is what makes this safe to run on a thread of its own.
    pub fn seed_agent_state(
        &mut self,
        pane: &PaneId,
        state: AgentState,
        expected: Option<AgentState>,
    ) -> Vec<Change> {
        let Some(held) = self.panes.get_mut(pane) else { return Vec::new() };
        let from = held.agent_state;
        if Some(from) != expected || from == state {
            return Vec::new();
        }
        held.agent_state = state;
        vec![Change::AgentStateChanged { pane: pane.clone(), from, to: state }]
    }
}
