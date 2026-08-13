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
    AgentStateChanged {
        pane: PaneId,
        state: AgentState,
        /// The backend's session-wide ordering stamp, where it has one. Tracked so a
        /// later jump reveals transitions that happened while nobody was listening.
        seq: Option<u64>,
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
    /// The backend's agent sequence skipped values, so transitions happened that this
    /// mirror never saw - possibly on panes it has never heard of. The only honest
    /// response is a fresh snapshot.
    AgentTransitionsMissed {
        expected: u64,
        saw: u64,
    },
}
