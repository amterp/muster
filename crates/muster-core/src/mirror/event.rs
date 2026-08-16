//! What a backend tells Muster changed.
//!
//! Upsert rather than created-and-updated, deliberately. `events.subscribe` replays the
//! current session as creation events, so a client that snapshots and then subscribes is
//! told every existing entity was just created (`docs/observations/herdr-0.8.0.md`
//! section 1). Collapsing the two makes convergence a property of the vocabulary rather
//! than a rule each adapter has to remember.
//!
//! **Where the two carry different information, they stay two.** That replay is a log of
//! past events rather than a statement of the present, so a creation says what an entity
//! was called when it was made and a rename says what it is called now - and a mirror that
//! read them as one lets a reconnect put an old name back. `TabRenamed` is that case; a
//! pane's name has no announcement at all and so is taken only from a snapshot (section 16).

use crate::AgentState;
use crate::mirror::backend::{Layout, Pane, PaneId, Tab, TabId, Workspace, WorkspaceId};

/// One thing a backend says happened.
///
/// Every variant carries absolute values rather than deltas, so applying one twice is
/// applying it once, and applying a stale one costs at most a redundant write.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    WorkspaceUpserted(Workspace),
    WorkspaceRemoved(WorkspaceId),
    TabUpserted(Tab),
    /// A tab somebody renamed, which is a different fact from a tab existing.
    ///
    /// Two events rather than one upsert because a backend may replay the first forever and
    /// the second is news. herdr's creation event carries the label the tab was made with -
    /// its position - so a replay of it puts back a number over whatever the tab is now
    /// called, and the caption then drops that number as a number and the row goes blank.
    /// Renaming is announced separately and only when it happens, so it can be trusted.
    TabRenamed {
        tab: TabId,
        label: String,
    },
    TabRemoved(TabId),
    PaneUpserted(Pane),
    /// A pane is gone, however it went. The backend distinguishes a pane a client closed
    /// from one whose program ended, and Muster does not: both mean the surface should
    /// stop existing, and keeping the difference would invite handling only one
    /// (`observations/herdr-0.8.0.md` section 10).
    PaneRemoved(PaneId),
    /// No sequence stamp, because no backend sends one on an event. herdr's
    /// `state_change_seq` appears in exactly one place in its whole schema - the `agents[]`
    /// of a `session.snapshot` - so ordering is something a snapshot carries and a stream
    /// does not (`observations/herdr-0.8.0.md` section 10).
    AgentStateChanged {
        pane: PaneId,
        state: AgentState,
    },
    AgentDetected {
        pane: PaneId,
        agent: String,
    },
    /// A whole tab's tree, as it stands now.
    ///
    /// Whole rather than incremental because that is how it arrives: herdr's
    /// `layout_updated` carries the entire tab in absolute values, so applying it twice is
    /// applying it once. It follows every pane change and **no** tab or workspace change,
    /// so nothing may treat it as the only structural signal
    /// (`observations/herdr-0.8.0.md` section 10).
    LayoutUpserted(Layout),
    /// A focus cursor moved. Each field is absolute and independent: `None` means this
    /// event says nothing about that cursor, not that the cursor was cleared.
    FocusMoved {
        workspace: Option<WorkspaceId>,
        tab: Option<TabId>,
        pane: Option<PaneId>,
    },
}

/// What applying an event actually changed.
///
/// Returned so that rendering costs the change rather than a walk of every pane
/// (`architecture.md`: fast is a feature, the per-event half). An event that changed
/// nothing - a replayed creation, a repeated state - produces nothing here, which is what
/// makes idempotence observable rather than merely intended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    PaneAdded(PaneId),
    /// A pane is gone. `cascaded` is true when nothing announced it: closing a tab or a
    /// workspace removes its panes silently, so this is the mirror's own inference rather
    /// than something the backend said.
    PaneRemoved {
        pane: PaneId,
        cascaded: bool,
    },
    AgentStateChanged {
        pane: PaneId,
        from: AgentState,
        to: AgentState,
    },
    /// What this pane is called has moved - its directory, or the harness detected in it.
    ///
    /// Not a state change: an agent that has just been recognized was already doing whatever
    /// it was doing, and a pane that changed directory is the same pane. It is reported
    /// because a list of panes names them by exactly these two things, and a name that never
    /// updates is a pane the user cannot find twice.
    PaneRelabelled(PaneId),
    TabAdded(TabId),
    /// What this tab is called has moved. The same shape as [`Change::PaneRelabelled`] and
    /// for the same reason: a caption that never updates is a tab somebody named and cannot
    /// find again.
    TabRelabelled(TabId),
    TabRemoved(TabId),
    /// This tab's tree is not the one it was. Carries the tab rather than the tree,
    /// because every reader has the mirror in hand and only some of them want to walk it.
    LayoutChanged(TabId),
    WorkspaceAdded(WorkspaceId),
    WorkspaceRemoved(WorkspaceId),
    FocusChanged,
    /// Between the last snapshot and this one, the backend ran more agent transitions than
    /// this mirror was told about - possibly on panes it has never heard of.
    ///
    /// An attention signal rather than a consistency one. The mirror is already correct by
    /// the time this is emitted, because it is emitted by the bootstrap that made it
    /// correct. What it says is that an agent may have asked for the user while nobody was
    /// listening, and a product whose reason to exist is routing attention should not let
    /// that pass silently (`README.md`, attention routing).
    AgentTransitionsMissed {
        expected: u64,
        saw: u64,
    },
}

impl Change {
    /// Whether this can have moved something composition names.
    ///
    /// Agent state and daemon focus cannot: one is a property of a pane that still exists,
    /// and the other is a cursor Muster writes and never reads. Everything else moves a tab
    /// or a pane, and both are things a region is holding on to.
    ///
    /// A false positive costs a reconcile and a republish that change nothing. A false
    /// negative leaves a region pointing at a tab the daemon has closed, which is why the
    /// unfamiliar case belongs on the true side.
    pub fn moves_structure(&self) -> bool {
        !matches!(
            self,
            Change::AgentStateChanged { .. }
                | Change::AgentTransitionsMissed { .. }
                | Change::PaneRelabelled(_)
                | Change::TabRelabelled(_)
                | Change::FocusChanged
        )
    }

    /// Whether what the window is showing would come out different.
    ///
    /// A superset of [`Change::moves_structure`], and the two are separate because they
    /// answer different questions. Composition has to be reconciled when something it names
    /// may have moved; the view and the roster have to be republished whenever anything in
    /// them would read differently - and a pane's name is in the roster without being
    /// anywhere composition can see.
    ///
    /// Agent state is the one thing in neither. It has a message of its own for exactly this
    /// reason: republishing the whole arrangement every time an agent blinked is the
    /// per-event cost the budget is drawn against, and a full window of agents is the common
    /// case rather than the rare one.
    pub fn republishes(&self) -> bool {
        self.moves_structure()
            || matches!(self, Change::PaneRelabelled(_) | Change::TabRelabelled(_))
    }

    /// The pane whose agent state the shell has to be told about, if any.
    ///
    /// A transition is the obvious case. A pane appearing is the one that is easy to miss,
    /// and was: a pane already working when Muster attaches has never transitioned, so a
    /// shell told only about transitions paints a busy agent as `unknown` until that agent
    /// happens to move again.
    ///
    /// The state is not carried here because the mirror already holds it, and a second copy
    /// travelling beside the pane id is a second copy to disagree.
    pub fn announces_agent_state(&self) -> Option<&PaneId> {
        match self {
            Change::AgentStateChanged { pane, .. } | Change::PaneAdded(pane) => Some(pane),
            _ => None,
        }
    }
}
