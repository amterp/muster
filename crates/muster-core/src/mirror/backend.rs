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

/// Which way a split divides its area.
///
/// Named for the arrangement it produces rather than for where a new pane went. herdr says
/// `right` and `down`, which describe the moment of splitting; a view has to know how to
/// lay two children out long after that moment, and translating at the seam means the
/// renderer never has to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    /// Children sit side by side. `first` is the left one.
    Columns,
    /// Children sit one above the other. `first` is the upper one.
    Rows,
}

/// One tab's pane tree.
///
/// Ratios rather than cells, because the cells are not about this window. herdr sizes its
/// rects for a terminal area of its own - a fixed 54x23 whether a client is attached or
/// not (`observations/herdr-0.8.0.md` section 13) - so the numbers it publishes describe
/// nobody's window and only the proportions survive the trip to a view rendering at its
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

    /// The same shape with two of its panes in each other's places.
    ///
    /// For an adapter whose backend has no leftward split and has to build one out of a
    /// rightward split and a swap. The daemon publishes the arrangement in between on its way
    /// past, and that arrangement is this, applied to the one it settled on - so reconstructing
    /// it is what lets the publish be recognized as already out of date rather than rendered.
    ///
    /// The shape does not move, only the two ids: a swap exchanges what sits in two places
    /// rather than rearranging the places.
    #[must_use]
    pub fn with_panes_exchanged(&self, one: &PaneId, other: &PaneId) -> LayoutNode {
        match self {
            LayoutNode::Pane(id) if id == one => LayoutNode::Pane(other.clone()),
            LayoutNode::Pane(id) if id == other => LayoutNode::Pane(one.clone()),
            LayoutNode::Pane(id) => LayoutNode::Pane(id.clone()),
            LayoutNode::Split { axis, ratio, first, second } => LayoutNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(first.with_panes_exchanged(one, other)),
                second: Box::new(second.with_panes_exchanged(one, other)),
            },
        }
    }
}

/// A tree on one line: `columns(w1:p1, rows(w1:p2, w1:p3@0.5)@0.5)`.
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

/// How one tab arranges its panes, and which of them the backend is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub tab: TabId,
    pub root: LayoutNode,
    /// The backend's focused pane in this tab. Rendered as a cursor, never used to route
    /// input: which pane Muster's keyboard feeds is view-local (`architecture.md`, cursors
    /// are written, not read).
    pub focused: Option<PaneId>,
    /// The pane filling the whole tab, when one is.
    ///
    /// herdr spells this as a flag beside the layout's focused pane, and keeps publishing
    /// every pane's ordinary unzoomed rect while it is set - so a view that renders what it
    /// is handed paints the whole tree while the daemon is showing one pane
    /// (`observations/herdr-0.8.0.md` section 13). Resolved to the pane itself here so that
    /// the question a view actually asks has an answer it cannot skip.
    pub zoomed: Option<PaneId>,
}

impl Layout {
    /// The same tab with two of its panes in each other's places.
    ///
    /// Only the arrangement moves. Both cursors here name a pane rather than a place, and a
    /// pane carries its focus and its zoom with it when it moves, so neither changes.
    #[must_use]
    pub fn with_panes_exchanged(&self, one: &PaneId, other: &PaneId) -> Layout {
        Layout {
            tab: self.tab.clone(),
            root: self.root.with_panes_exchanged(one, other),
            focused: self.focused.clone(),
            zoomed: self.zoomed.clone(),
        }
    }
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    /// One per tab that has one. A tab with no readable layout is absent rather than
    /// empty, so a view keeps whatever it had instead of blanking.
    pub layouts: Vec<Layout>,
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
