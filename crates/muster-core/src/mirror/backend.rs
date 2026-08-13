//! Muster's nouns for what a session backend holds.
//!
//! Named for what Muster needs rather than for what herdr happens to offer
//! (`docs/architecture.md`, the vocabulary). Nothing herdr-shaped reaches here: the
//! adapter translates into these types, and a second backend would translate into the
//! same ones.

use crate::AgentState;

/// The three ids are separate types because they are all strings shaped `w1:p1` and
/// `w1:t1`, and passing one where another belongs is a lookup that quietly finds
/// nothing. A pane that never appears is much harder to debug than a type error.
macro_rules! id_type {
    ($name:ident, $what:literal) => {
        #[doc = concat!("Identifies one ", $what, ", as the backend spells it. Opaque: Muster never parses it.")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> $name {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> $name {
                $name(id.to_string())
            }
        }
    };
}

id_type!(WorkspaceId, "workspace");
id_type!(TabId, "tab");
id_type!(PaneId, "pane");

/// One daemon-owned terminal, and what its agent is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: PaneId,
    pub tab: TabId,
    pub workspace: WorkspaceId,
    pub agent_state: AgentState,
    /// The harness the backend recognized, if it recognized one. `None` is not
    /// `AgentState::Unknown`: a pane can run no agent at all and be perfectly idle.
    pub agent: Option<String>,
    pub cwd: String,
}

/// The unit that owns one pane tree. Trees hang off tabs, not workspaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub workspace: WorkspaceId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub label: String,
}

/// The backend's three focus cursors.
///
/// Read for display, never for routing: which pane Muster's keyboard feeds is view-local,
/// so another client moving daemon focus must not yank it (`architecture.md`, cursors are
/// written, not read).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    pub workspace: Option<WorkspaceId>,
    pub tab: Option<TabId>,
    pub pane: Option<PaneId>,
}

/// Everything a backend says is true right now, as one answer.
///
/// What a mirror bootstraps from, and what it rebuilds from after any gap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    pub focus: Focus,
    /// The highest agent-state sequence the backend has issued, if it issues one. Lets a
    /// later gap be noticed rather than merely survived (`architecture.md`, event model).
    pub agent_state_seq: Option<u64>,
}

/// How much of the backend's truth Muster currently has.
///
/// State rather than an error path: a stale mirror still renders, labeled
/// (`architecture.md`, degradation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Health {
    /// Live control plane. What the mirror says is what the daemon said.
    Connected,
    /// The control plane went quiet or dropped. The last mirror is still the best
    /// available answer, and it is now a guess about the present.
    Stale,
    /// Nothing is connected, and reconnecting means a fresh snapshot. The default,
    /// because a mirror that has never spoken to a daemon knows nothing, and starting at
    /// `Connected` would render an empty session as a real one.
    #[default]
    Disconnected,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Health::Connected => "connected",
            Health::Stale => "stale",
            Health::Disconnected => "disconnected",
        }
    }
}
