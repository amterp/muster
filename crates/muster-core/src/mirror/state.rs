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
use crate::mirror::backend::{Health, LayoutNode, Pane, PaneId, Snapshot, Tab, TabId};
use crate::mirror::event::{BackendEvent, Change};
use crate::mirror::ordered::Ordered;

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
    /// The daemon is still bringing back its saved tabs.
    restoring: bool,
    health: Health,
    /// Why the health is what it is, for anyone who has to say so out loud. Empty when
    /// connected, because a live connection needs no excuse.
    ///
    /// Kept beside the health rather than only logged, because it is asked for outside the
    /// moment it happened: an event carries it to a shell that was listening, and a `Window`
    /// read has to answer a caller that was not. "stale" alone tells somebody their picture
    /// might be wrong and nothing about whether to wait or go looking.
    health_detail: String,
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
                }
            }
        }
        for id in previous_panes.keys() {
            if !self.panes.contains_key(id) {
                changes.push(Change::PaneRemoved(id.clone()));
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
        changes
    }

    /// Applies one event, and reports what it actually changed.
    pub fn apply(&mut self, event: BackendEvent) -> Vec<Change> {
        match event {
            BackendEvent::PaneOpened(pane) | BackendEvent::PaneChanged(pane) => {
                self.upsert_pane(pane)
            }
            BackendEvent::PaneClosed(id) => {
                self.unplaced.remove(&id);
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
                vec![Change::Restored(restored)]
            }
            BackendEvent::PasteHeld { pane, text } => vec![Change::PasteHeld { pane, text }],
            BackendEvent::ClipboardWrite { pane, text } => {
                vec![Change::ClipboardWrite { pane, text }]
            }
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

    pub fn agent_state(&self, id: &PaneId) -> Option<AgentState> {
        self.panes.get(id).map(|pane| pane.agent_state)
    }
}

/// Whether what a pane is called has moved: the things a list of panes names it by.
fn relabelled(before: &Pane, now: &Pane) -> bool {
    before.cwd != now.cwd
        || before.agent != now.agent
        || before.name != now.name
        || before.title != now.title
}

/// Whether what is known of a pane's agent beyond its state has moved.
fn described(before: &Pane, now: &Pane) -> bool {
    before.facts != now.facts
        || before.reported != now.reported
        || before.unreadable != now.unreadable
}

/// Whether a tab's tree or its zoom has moved.
fn arrangement_moved(before: &Tab, now: &Tab) -> bool {
    before.root != now.root || before.zoomed != now.zoomed
}
