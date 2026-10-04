//! A convergent picture of what a daemon holds.
//!
//! The core owns a mirror: a derived, disposable cache of daemon structure, bootstrapped
//! from an authoritative snapshot plus the events after it, rebuilt after any gap, never
//! patched across one (`docs/architecture.md`, ownership of truth).
//!
//! Pure by construction - no sockets, no threads, no clock. That is not tidiness: it is
//! what lets the whole of this behavior be judged by cases rather than by staging a daemon
//! into each state (`docs/testing.md`).

use std::collections::BTreeMap;

use crate::AgentState;
use crate::attention::HumanNotice;
use crate::mirror::backend::{Health, LayoutNode, Pane, PaneId, Progress, Snapshot, Tab, TabId};
use crate::mirror::event::{BackendEvent, Change};
use crate::mirror::ordered::Ordered;
use crate::mirror::restart::Restart;

/// What the daemon says is true, as far as this mirror knows.
///
/// Maps rather than vectors, ordered by arrival rather than hashed: iteration order is part of
/// what the log and the corpus compare, and a picture that reorders itself between runs is one
/// nobody can diff.
#[derive(Debug, Default)]
pub struct Mirror {
    tabs: Ordered<TabId, Tab>,
    /// Every pane a tree names, each with the tab whose tree that is.
    panes: Ordered<PaneId, Pane>,
    /// Panes the daemon has opened and no tree names yet.
    ///
    /// A pane is opened before the tree that places it changes, so for a moment it belongs to
    /// no tab. Held here rather than shown, because everything that draws or lists a pane
    /// does so by its tab; it is added when a tree names it.
    unplaced: BTreeMap<PaneId, Pane>,
    /// What each pane's program last said of its progress. Not on the daemon's record: it
    /// arrives as an effect, which no snapshot repeats, so a new connection starts without.
    progress: BTreeMap<PaneId, Progress>,
    /// The daemon is still bringing back its saved tabs.
    restoring: bool,
    /// What waits for the human, by group, as the daemon last said.
    human: BTreeMap<String, HumanNotice>,
    health: Health,
    /// Why the health is what it is, for anyone who has to say so out loud. Empty when
    /// connected, because a live connection needs no excuse.
    ///
    /// Kept beside the health rather than only logged, because it is asked for outside the
    /// moment it happened: an event carries it to a shell that was listening, and a `Window`
    /// read has to answer a caller that was not. "stale" alone tells somebody their picture
    /// might be wrong and nothing about whether to wait or go looking.
    health_detail: String,
    /// Which run of the daemon the mirror holds, once it has held one.
    instance: Option<u64>,
    /// The panes a previous run held, kept while a new run is still restoring: what came back
    /// can only be told once it has finished.
    before_restart: Option<Vec<Pane>>,
    /// What the daemon's last restart cost, kept for anybody who asks later.
    last_restart: Option<Restart>,
    /// Whether that restart still needs somebody ([`Restart::outstanding`]).
    restart_outstanding: bool,
}

impl Mirror {
    pub fn new() -> Mirror {
        Mirror::default()
    }

    /// Replaces everything with what the daemon just said, and reports what moved.
    ///
    /// Used for the first connection and for every reconnection. Reporting the difference
    /// rather than "everything changed" is what lets a reconnect update the view without
    /// repainting panes that were never affected.
    pub fn bootstrap(&mut self, snapshot: Snapshot) -> Vec<Change> {
        let previous_panes = std::mem::take(&mut self.panes);
        let previous_tabs = std::mem::take(&mut self.tabs);
        self.unplaced.clear();
        let previous_progress = std::mem::take(&mut self.progress);
        let previous_human = std::mem::replace(&mut self.human, snapshot.human);
        let new_run = self.instance.is_some_and(|held| held != snapshot.instance);
        self.instance = Some(snapshot.instance);
        let restored_from_file = snapshot.restored_from_file;
        self.restoring = snapshot.restoring;
        self.health = Health::Connected;
        self.health_detail.clear();

        self.tabs = snapshot.tabs.into_iter().map(|tab| (tab.id.clone(), tab)).collect();
        let placed = self.placements();
        for mut pane in snapshot.panes {
            match placed.get(&pane.id) {
                Some(tab) => {
                    pane.tab = tab.clone();
                    self.panes.insert(pane.id.clone(), pane);
                }
                None => {
                    self.unplaced.insert(pane.id.clone(), pane);
                }
            }
        }

        let mut changes = Vec::new();
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
                Some(before) => {
                    if before.agent_state != pane.agent_state {
                        changes.push(Change::AgentStateChanged {
                            pane: id.clone(),
                            from: before.agent_state,
                            to: pane.agent_state,
                        });
                    }
                    if before.finished_unseen != pane.finished_unseen {
                        changes.push(Change::FinishedUnseen {
                            pane: id.clone(),
                            unseen: pane.finished_unseen,
                        });
                    }
                    if relabelled(before, pane) {
                        changes.push(Change::PaneRelabelled(id.clone()));
                    }
                    if described(before, pane) {
                        changes.push(Change::AgentDescribed(id.clone()));
                    }
                    if previous_progress.contains_key(id) {
                        changes.push(Change::ProgressChanged(id.clone()));
                    }
                }
            }
        }
        for id in previous_panes.keys() {
            if !self.panes.contains_key(id) {
                changes.push(Change::PaneRemoved(id.clone()));
            }
        }
        if new_run {
            let before: Vec<Pane> = previous_panes.values().cloned().collect();
            if self.restoring {
                self.before_restart = Some(before);
            } else {
                self.before_restart = None;
                changes.extend(self.restarted(&before, restored_from_file));
            }
        }
        // After the panes, because a tree names them: a reader told the arrangement first
        // would be handed a tree referring to a pane it has not been told exists. A tab that
        // went away is a TabRemoved and needs no second announcement about its tree.
        for (id, tab) in self.tabs.iter() {
            if let Some(before) = previous_tabs.get(id) {
                if before.label != tab.label {
                    changes.push(Change::TabRelabelled(id.clone()));
                }
                if arrangement_moved(before, tab) {
                    changes.push(Change::LayoutChanged(id.clone()));
                }
            } else {
                changes.push(Change::LayoutChanged(id.clone()));
            }
        }
        let groups: std::collections::BTreeSet<&String> =
            previous_human.keys().chain(self.human.keys()).collect();
        for group in groups {
            if previous_human.get(group) != self.human.get(group) {
                changes.push(Change::HumanNoticed(group.clone()));
            }
        }
        changes.extend(self.restart_settled());
        changes
    }

    /// What waits for the human in `group`, as the daemon last said: a count of 0 once they read
    /// it, and nothing once they leave it.
    pub fn human_notice(&self, group: &str) -> Option<&HumanNotice> {
        self.human.get(group)
    }

    /// Every group the human is in, with what waits for them there.
    pub fn human_notices(&self) -> impl Iterator<Item = (&String, &HumanNotice)> {
        self.human.iter()
    }

    /// Applies one event, and reports what it actually changed.
    pub fn apply(&mut self, event: BackendEvent) -> Vec<Change> {
        let mut changes = self.apply_event(event);
        changes.extend(self.restart_settled());
        changes
    }

    fn apply_event(&mut self, event: BackendEvent) -> Vec<Change> {
        match event {
            BackendEvent::PaneOpened(pane) | BackendEvent::PaneChanged(pane) => {
                self.upsert_pane(pane)
            }
            BackendEvent::PaneClosed(id) => {
                self.unplaced.remove(&id);
                self.progress.remove(&id);
                match self.panes.remove(&id) {
                    Some(_) => vec![Change::PaneRemoved(id)],
                    None => Vec::new(),
                }
            }
            BackendEvent::TabOpened(tab) | BackendEvent::TabChanged(tab) => self.upsert_tab(tab),
            BackendEvent::TabClosed(id) => match self.tabs.remove(&id) {
                Some(_) => vec![Change::TabRemoved(id)],
                None => Vec::new(),
            },
            BackendEvent::Restored(restored) => {
                self.restoring = false;
                let mut changes = vec![Change::Restored(restored)];
                // Only a restore says it has finished, so the panes came back from the file.
                if let Some(before) = self.before_restart.take() {
                    changes.extend(self.restarted(&before, true));
                }
                changes
            }
            BackendEvent::PasteHeld { pane, text } => vec![Change::PasteHeld { pane, text }],
            BackendEvent::ClipboardWrite { pane, text } => {
                vec![Change::ClipboardWrite { pane, text }]
            }
            BackendEvent::Bell { pane } if self.panes.contains_key(&pane) => {
                vec![Change::Rang(pane)]
            }
            BackendEvent::Notified { pane, title, body } if self.panes.contains_key(&pane) => {
                vec![Change::Notified { pane, title, body }]
            }
            BackendEvent::Progress { pane, progress } if self.panes.contains_key(&pane) => {
                let before = match progress {
                    Some(progress) => self.progress.insert(pane.clone(), progress),
                    None => self.progress.remove(&pane),
                };
                if before == progress { Vec::new() } else { vec![Change::ProgressChanged(pane)] }
            }
            BackendEvent::HumanNotice { group, notice } => {
                let before = if notice.listed() {
                    self.human.insert(group.clone(), notice.clone())
                } else {
                    self.human.remove(&group)
                };
                let after = notice.listed().then_some(notice);
                if before == after { Vec::new() } else { vec![Change::HumanNoticed(group)] }
            }
            // For a pane no tree names yet, which draws nowhere.
            BackendEvent::Bell { .. }
            | BackendEvent::Notified { .. }
            | BackendEvent::Progress { .. } => Vec::new(),
        }
    }

    fn upsert_pane(&mut self, mut pane: Pane) -> Vec<Change> {
        let id = pane.id.clone();
        let Some(before) = self.panes.get(&id) else {
            // Placed only once a tree names it; until then it waits here, and a later record
            // replaces the one waiting.
            self.unplaced.insert(id, pane);
            return Vec::new();
        };
        pane.tab = before.tab.clone();
        let mut changes = Vec::new();
        if before.agent_state != pane.agent_state {
            changes.push(Change::AgentStateChanged {
                pane: id.clone(),
                from: before.agent_state,
                to: pane.agent_state,
            });
        }
        if before.finished_unseen != pane.finished_unseen {
            changes.push(Change::FinishedUnseen { pane: id.clone(), unseen: pane.finished_unseen });
        }
        if relabelled(before, &pane) {
            changes.push(Change::PaneRelabelled(id.clone()));
        }
        if described(before, &pane) {
            changes.push(Change::AgentDescribed(id.clone()));
        }
        self.panes.insert(id, pane);
        changes
    }

    fn upsert_tab(&mut self, tab: Tab) -> Vec<Change> {
        let id = tab.id.clone();
        let mut changes = Vec::new();
        let moved = match self.tabs.get(&id) {
            None => {
                changes.push(Change::TabAdded(id.clone()));
                true
            }
            Some(before) => {
                if before.label != tab.label {
                    changes.push(Change::TabRelabelled(id.clone()));
                }
                arrangement_moved(before, &tab)
            }
        };
        let named: Vec<PaneId> = tab.root.panes().into_iter().cloned().collect();
        self.tabs.insert(id.clone(), tab);
        // Panes this tree names, placed before the tree is reported, so that nothing hears of
        // an arrangement holding a pane it has not been told exists. A pane already held
        // elsewhere has moved here, and takes this tab.
        for pane in named {
            if let Some(mut waiting) = self.unplaced.remove(&pane) {
                waiting.tab = id.clone();
                self.panes.insert(pane.clone(), waiting);
                changes.push(Change::PaneAdded(pane));
            } else if let Some(held) = self.panes.get_mut(&pane) {
                held.tab = id.clone();
            }
        }
        if moved {
            changes.push(Change::LayoutChanged(id));
        }
        changes
    }

    /// Which tab's tree names each pane.
    fn placements(&self) -> BTreeMap<PaneId, TabId> {
        let mut placed = BTreeMap::new();
        for tab in self.tabs.values() {
            for pane in tab.root.panes() {
                placed.insert(pane.clone(), tab.id.clone());
            }
        }
        placed
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

    /// Whether the daemon is still bringing back its saved tabs, so that holding nothing does
    /// not yet mean it is empty.
    pub fn restoring(&self) -> bool {
        self.restoring
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

    pub fn panes_in_tab<'a>(&'a self, tab: &'a TabId) -> impl Iterator<Item = &'a Pane> {
        self.panes.values().filter(move |pane| &pane.tab == tab)
    }

    /// How a tab arranges its panes, if the mirror holds the tab.
    pub fn tree(&self, tab: &TabId) -> Option<&LayoutNode> {
        self.tabs.get(tab).map(|tab| &tab.root)
    }

    /// What a pane's program last said of its progress, if it is still saying anything.
    pub fn progress(&self, id: &PaneId) -> Option<Progress> {
        self.progress.get(id).copied()
    }

    /// What the daemon's last restart cost, if the mirror has seen one.
    pub fn last_restart(&self) -> Option<&Restart> {
        self.last_restart.as_ref()
    }

    /// Works out what a restart cost against the panes held before it, and keeps it.
    /// Whether the daemon's last restart still needs somebody.
    pub fn restart_outstanding(&self) -> bool {
        self.restart_outstanding
    }

    /// Works out what a restart cost against the panes held before it, and keeps it. Said
    /// only when it needs somebody; otherwise it is kept for `muster window` alone.
    fn restarted(&mut self, before: &[Pane], from_file: bool) -> Option<Change> {
        let restart = Restart::between(
            before,
            |pane| self.panes.contains_key(pane) || self.unplaced.contains_key(pane),
            from_file,
        )?;
        self.last_restart = Some(restart.clone());
        self.restart_outstanding = self.restart_needs_somebody();
        self.restart_outstanding.then_some(Change::Restarted(restart))
    }

    fn restart_needs_somebody(&self) -> bool {
        let Some(restart) = &self.last_restart else { return false };
        let pane = |id: &PaneId| self.panes.get(id).or_else(|| self.unplaced.get(id));
        restart.outstanding(
            |id| pane(id).map(|held| held.agent.as_deref().filter(|agent| !agent.is_empty())),
            !self.panes.is_empty() || !self.unplaced.is_empty(),
        )
    }

    /// Says once that an outstanding restart no longer needs anybody.
    fn restart_settled(&mut self) -> Option<Change> {
        if !self.restart_outstanding || self.restart_needs_somebody() {
            return None;
        }
        self.restart_outstanding = false;
        Some(Change::RestartSettled)
    }

    pub fn agent_state(&self, id: &PaneId) -> Option<AgentState> {
        self.panes.get(id).map(|pane| pane.agent_state)
    }
}

/// Whether what a list of panes says of a pane has moved: the things it names it by, and
/// whether its agent can be compacted, which the list's menus offer on.
fn relabelled(before: &Pane, now: &Pane) -> bool {
    before.cwd != now.cwd
        || before.agent != now.agent
        || before.compactable != now.compactable
        || before.name != now.name
        || before.title != now.title
}

/// Whether what is known of a pane's agent beyond its state has moved.
fn described(before: &Pane, now: &Pane) -> bool {
    before.facts != now.facts
        || before.reported != now.reported
        || before.unreadable != now.unreadable
        || before.adapter != now.adapter
}

/// Whether a tab's tree or its zoom has moved.
fn arrangement_moved(before: &Tab, now: &Tab) -> bool {
    before.root != now.root || before.zoomed != now.zoomed
}
