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

use crate::AgentState;
use crate::mirror::backend::{
    Focus, Health, Pane, PaneId, Snapshot, Tab, TabId, Workspace, WorkspaceId,
};
use crate::mirror::event::{BackendEvent, Change};

/// What the backend says is true, as far as this mirror knows.
///
/// Maps rather than vectors, ordered rather than hashed: iteration order is part of what
/// the log and the corpus compare, and a picture that reorders itself between runs is one
/// nobody can diff.
#[derive(Debug, Default)]
pub struct Mirror {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    tabs: BTreeMap<TabId, Tab>,
    panes: BTreeMap<PaneId, Pane>,
    focus: Focus,
    health: Health,
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
        let previous_focus = std::mem::replace(&mut self.focus, snapshot.focus);

        self.workspaces = snapshot.workspaces.into_iter().map(|w| (w.id.clone(), w)).collect();
        self.tabs = snapshot.tabs.into_iter().map(|t| (t.id.clone(), t)).collect();
        self.panes = snapshot.panes.into_iter().map(|p| (p.id.clone(), p)).collect();
        self.health = Health::Connected;
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
        for (id, pane) in &self.panes {
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

    fn apply_inner(&mut self, event: BackendEvent) -> Vec<Change> {
        match event {
            // Upserting an existing workspace or tab reports nothing, even when its label
            // moved. Nothing renders labels yet, and a Change variant with no consumer is
            // a guess at what a reader will want rather than an answer to one.
            BackendEvent::WorkspaceUpserted(workspace) => {
                let id = workspace.id.clone();
                if self.workspaces.insert(id.clone(), workspace).is_some() {
                    Vec::new()
                } else {
                    vec![Change::WorkspaceAdded(id)]
                }
            }
            BackendEvent::WorkspaceRemoved(id) => self.remove_workspace(&id),
            BackendEvent::TabUpserted(tab) => {
                let id = tab.id.clone();
                if self.tabs.insert(id.clone(), tab).is_some() {
                    Vec::new()
                } else {
                    vec![Change::TabAdded(id)]
                }
            }
            BackendEvent::TabRemoved(id) => self.remove_tab(&id),
            BackendEvent::PaneUpserted(pane) => {
                let id = pane.id.clone();
                match self.panes.insert(id.clone(), pane) {
                    None => vec![Change::PaneAdded(id)],
                    Some(before) => {
                        let now = self.panes[&id].agent_state;
                        if before.agent_state == now {
                            Vec::new()
                        } else {
                            vec![Change::AgentStateChanged {
                                pane: id,
                                from: before.agent_state,
                                to: now,
                            }]
                        }
                    }
                }
            }
            BackendEvent::PaneRemoved(id) => self.remove_pane(&id, false),
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
        if self.panes.remove(id).is_none() {
            // A removal for something already gone is a no-op rather than an error. The
            // subscription replays, reconnects re-snapshot, and both routinely say a
            // thing is gone that this mirror already dropped.
            return Vec::new();
        }
        self.forget_focus();
        vec![Change::PaneRemoved { pane: id.clone(), cascaded }]
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

    pub fn mark_stale(&mut self) {
        self.health = Health::Stale;
    }

    pub fn mark_disconnected(&mut self) {
        self.health = Health::Disconnected;
    }

    pub fn health(&self) -> Health {
        self.health
    }

    pub fn pane(&self, id: &PaneId) -> Option<&Pane> {
        self.panes.get(id)
    }

    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
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

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    pub fn agent_state(&self, id: &PaneId) -> Option<AgentState> {
        self.panes.get(id).map(|pane| pane.agent_state)
    }
}
