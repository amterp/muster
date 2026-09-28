//! Muster's nouns for what a session daemon holds.
//!
//! The daemon's own records, in the core's types: tabs that carry their pane tree and a
//! label with a generation, and panes that are whole records, facts and all (MIP-3, section 2).
//! The client translates its protocol into these, and the core depends on no daemon crate.

use std::collections::BTreeMap;

use crate::AgentState;

/// The ids are separate types because they are all short strings, and passing one where
/// another belongs is a lookup that quietly finds nothing. A pane that never appears is much
/// harder to debug than a type error.
macro_rules! id_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[doc = ""]
        #[doc = "Opaque: Muster never parses it."]
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

        // The owned half, which the name registry needs: it is generic over these types and
        // draws a `String` it then has to make an id of.
        impl From<String> for $name {
            fn from(id: String) -> $name {
                $name(id)
            }
        }
    };
}

pub(crate) use id_type;

id_type!(
    TabId,
    "Identifies one tab by its Muster name (`t1w3r07bsd`), minted by Muster and passed in the \
     request that makes it (see [`crate::names`]). A tab grouped across two machines is one \
     name held on both daemons."
);
id_type!(
    PaneId,
    "Identifies one pane by its Muster name (`p1w3r07bsd`), minted by Muster and passed in the \
     request that makes it, so the pane's process knows it from birth as `MUSTER_PANE`."
);

/// One daemon-owned terminal, and what its agent is doing.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub id: PaneId,
    /// The tab whose tree holds this pane. The daemon's record does not carry it, so the
    /// mirror sets it from the tab trees it holds, and whatever a backend passes is replaced.
    pub tab: TabId,
    pub agent_state: AgentState,
    /// The agent stopped working or waiting on somebody, and no window with the keyboard has
    /// shown the pane since. The daemon's fact, cleared by a window reporting it seen; a pane
    /// carrying it is `done` (`crate::attention`).
    pub finished_unseen: bool,
    /// The harness the daemon recognized, if it recognized one. `None` is not
    /// `AgentState::Unknown`: a pane can run no agent at all and be perfectly idle.
    pub agent: Option<String>,
    pub cwd: String,
    /// What a person called this pane, if anybody has. Durable identity: the daemon writes it
    /// down, so it comes back after a daemon restart.
    pub name: Option<String>,
    /// What the program in the pane last called itself. Volatile status: a restart loses it,
    /// because the process that would set it again is new.
    pub title: Option<String>,
    /// The command the pane was started with, if it was started with one rather than a shell.
    pub command: Option<String>,
    /// What the agent in the pane has said about itself.
    pub facts: AgentFacts,
    /// Whether `agent_state` is the agent's own report rather than what the daemon read off its
    /// screen.
    pub reported: bool,
    /// Whether the daemon's rules have stopped reading this agent's screen, so that its state
    /// comes only from what the agent reports.
    pub unreadable: bool,
}

impl Pane {
    /// The state a window paints for this pane before attention lays `done` over it: `waiting`
    /// for an idle agent that said it is waiting on its own work, and otherwise what the daemon
    /// said.
    pub fn presented_state(&self) -> AgentState {
        if self.agent_state == AgentState::Idle && self.facts.waiting.is_some() {
            AgentState::Waiting
        } else {
            self.agent_state
        }
    }
}

/// What a program in a pane last said of its own progress (OSC 9;4), until it takes it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub state: ProgressState,
    /// How far along, from 0 to 100, when the program said.
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Running,
    /// The work failed, and the program is saying so.
    Error,
    /// Running, with no idea how far along. Some agents say this for as long as they work.
    Indeterminate,
    Paused,
}

impl ProgressState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressState::Running => "running",
            ProgressState::Error => "error",
            ProgressState::Indeterminate => "indeterminate",
            ProgressState::Paused => "paused",
        }
    }
}

/// What an agent reports about itself, in its own words (MIP-3, section 2). Never read off its
/// screen, and forgotten when the pane's agent changes or leaves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentFacts {
    /// How much of its context window is in use, from 0 to 100.
    pub context_used: Option<f32>,
    /// How many sub-agents it has running.
    pub subagents: u32,
    pub model: Option<String>,
    /// What its session has cost, in US dollars.
    pub cost_usd: Option<f64>,
    /// Anything else it chose to say, by name.
    pub other: BTreeMap<String, String>,
    /// What it ended its turn to wait on, work it started itself: it has not finished.
    pub waiting: Option<String>,
}

/// A tab: its tree over this machine's panes, what it is called, and which pane is zoomed.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: TabId,
    /// What somebody named it. `None` is a tab nobody named, which a caption numbers.
    pub label: Option<String>,
    /// How many times the label has been set. A tab grouped across machines has a label on
    /// every part, and the highest generation is the current one.
    pub generation: u64,
    pub root: LayoutNode,
    pub zoomed: Option<PaneId>,
}

/// Which way a split divides its area.
///
/// Named for the arrangement it produces rather than for where a new pane went: a view has to
/// know how to lay two children out long after the moment of splitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    /// Children sit side by side. `first` is the left one.
    Columns,
    /// Children sit one above the other. `first` is the upper one.
    Rows,
}

/// One tab's pane tree.
///
/// Ratios rather than cells: the daemon's tree is proportions, and a view lays them out at its
/// own size.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        /// The first child's share of the area, between 0 and 1.
        ///
        /// Compared exactly for change detection, which is safe because it is never
        /// computed here: it arrives from one backend and is stored unchanged, so two
        /// reads of an unmoved divider are the same bits. A ratio Muster sends *out* is
        /// computed from a drag and never read back.
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// Every pane in the tree, in reading order.
    ///
    /// Allocates, and is meant to: this is called when structure changes, never per byte
    /// or per keystroke. What it is for is the check nothing else can do - whether the
    /// tree and the mirror's pane list describe the same session.
    pub fn panes(&self) -> Vec<&PaneId> {
        let mut found = Vec::new();
        self.collect_panes(&mut found);
        found
    }

    fn collect_panes<'a>(&'a self, found: &mut Vec<&'a PaneId>) {
        match self {
            LayoutNode::Pane(id) => found.push(id),
            LayoutNode::Split { first, second, .. } => {
                first.collect_panes(found);
                second.collect_panes(found);
            }
        }
    }
}

/// A tree on one line: `columns(p1, rows(p2, p3@0.5)@0.5)`.
///
/// Exists for the run log, where "the layout changed" is useless and the shape it changed
/// to is the whole answer, and reused by the conformance drivers - a reviewer deciding
/// whether an expectation is right can hold this in their head, and cannot hold four
/// screens of nested JSON.
impl std::fmt::Display for LayoutNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutNode::Pane(id) => write!(f, "{id}"),
            LayoutNode::Split { axis, ratio, first, second } => {
                let axis = match axis {
                    SplitAxis::Columns => "columns",
                    SplitAxis::Rows => "rows",
                };
                write!(f, "{axis}({first}, {second}@{ratio})")
            }
        }
    }
}

/// Everything a daemon holds, as of one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The last event this snapshot includes. Events after it follow in order.
    pub seq: u64,
    /// Which run of the daemon this is. A new one means everything was rebuilt from its
    /// saved state, and nothing about the previous run carries over.
    pub instance: u64,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    /// The daemon is still bringing back its saved tabs, which arrive as events. Until it
    /// says it has finished, a daemon holding nothing is not an empty one.
    pub restoring: bool,
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
