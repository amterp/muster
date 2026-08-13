//! What a backend tells Muster changed.
//!
//! Upsert rather than created-and-updated, deliberately. `events.subscribe` replays the
//! current session as synthetic creation events, so a client that snapshots and then
//! subscribes is told every existing entity was just created
//! (`docs/observations/herdr-0.8.0.md` section 1). Collapsing the two makes convergence a
//! property of the vocabulary rather than a rule each adapter has to remember, and there
//! is no other information in the distinction: both carry the whole entity.

use crate::AgentState;
use crate::mirror::backend::{Pane, PaneId, Tab, TabId, Workspace, WorkspaceId};

/// One thing a backend says happened.
///
/// Every variant carries absolute values rather than deltas, so applying one twice is
/// applying it once, and applying a stale one costs at most a redundant write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    WorkspaceUpserted(Workspace),
    WorkspaceRemoved(WorkspaceId),
    TabUpserted(Tab),
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
    TabAdded(TabId),
    TabRemoved(TabId),
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
