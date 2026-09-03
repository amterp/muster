//! Muster's nouns for what a session backend holds.
//!
//! Named for what Muster needs rather than for what herdr happens to offer
//! (`docs/architecture.md`, the vocabulary). Nothing herdr-shaped reaches here: the
//! adapter translates into these types, and a second backend would translate into the
//! same ones.

use std::collections::BTreeSet;

use crate::AgentState;

/// The ids are separate types because they are all strings shaped `w1:p1` and `w1:t1`, and
/// passing one where another belongs is a lookup that quietly finds nothing. A pane that
/// never appears is much harder to debug than a type error.
///
/// [`crate::names`] mints two of them and builds its own backend-spelled pair from this same
/// macro, which is why it is importable rather than private to this module: the opaqueness
/// below is the invariant all five share, and one place that states it is fewer than five.
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

id_type!(WorkspaceId, "Identifies one workspace, as the backend spells it.");
id_type!(
    TabId,
    "Identifies one tab, as *Muster* spells it - a name Muster minted and the adapter \
     translates at the wire (see [`crate::names`]).\n\nNamed so that it can be addressed, \
     which is the half of a pane's reason that applies here: nothing has to tell a tab which \
     tab it is, but a backend's tab id is `w1:t1` on every machine, so it stops being an \
     answer the moment a window shows two."
);
id_type!(
    PaneId,
    "Identifies one pane, as *Muster* spells it - a name Muster minted and the adapter \
     translates at the wire (see [`crate::names`]). Named because Muster has to be able to \
     tell a pane which pane it is and a backend's id arrives too late for that."
);

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
    /// What a person called this pane, if anybody has. Durable identity: the backend
    /// writes it down, so it comes back after a daemon restart.
    pub name: Option<String>,
    /// What the program in the pane last called itself, with any activity glyph already
    /// removed by the backend. Volatile status: a restart loses it, because the process
    /// that would set it again is new (`observations/herdr-0.8.0.md` section 16).
    pub title: Option<String>,
    /// How many times the backend has changed this pane's [`Pane::title`].
    ///
    /// The ordering between two payloads about one pane, and it is the backend's own rather
    /// than a clock. Needed because a backend may replay an event the mirror has already
    /// moved past, and a title that goes backwards is a row saying an agent is doing
    /// something it finished ten minutes ago.
    ///
    /// **The title and nothing else.** herdr's `revision` moves on a changed stripped title
    /// and on no other event - not an agent changing state, and not a rename
    /// (`~/src/herdr/src/terminal/state.rs`, and `observations/herdr-0.8.0.md` sections 10
    /// and 16). So it orders this one field, and reading it as a general freshness stamp
    /// would be wrong in the one case that matters: a rename leaves it untouched.
    ///
    /// Zero for a backend that counts nothing, which makes every payload equally current -
    /// the same behaviour as before this existed.
    pub revision: u64,
}

/// What a pane has printed, as far back as the backend would go.
///
/// Asked for rather than followed, on the same terms as [`Viewport`] below: a pane's output
/// never enters the core (`architecture.md`, control plane and data plane), so this is the
/// one place its text is read at all - and it is read at the moment somebody asks rather than
/// held, because holding it would be a copy of every pane's history going stale between reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneText {
    /// Newest row last, the way the pane draws it.
    pub text: String,

    /// Whether the pane holds history this read never reached.
    ///
    /// The backend's own answer rather than a guess from the row count, for the reason
    /// [`crate::find::Found`] carries the same flag: a thousand rows back may be all a pane
    /// has or a fifth of it, and only the backend knows which. [`PaneText::tail`] can also
    /// set it, because a caller that asked for forty rows out of a hundred did not reach the
    /// other sixty either.
    pub truncated: bool,
}

impl PaneText {
    /// The last `rows` rows of what a backend handed back, or all of it for zero.
    ///
    /// A count rather than a ceiling, and that distinction is the whole of why this exists.
    /// A backend counts rows in the grid it draws, and the bottom of an idle pane is the
    /// blank remainder of its viewport - so asking a daemon for the last forty rows of a
    /// pane sitting at a prompt buys forty blank ones, which trim away to nothing. Blank is
    /// byte-identical to a pane that has printed nothing, and it cost this card's author a
    /// near-miss on closing a shell with twenty-four lines on it.
    ///
    /// So Muster asks for as far back as the backend will go and counts here, where a row is
    /// a row of text rather than a cell in a grid. Rows are split the way a search splits
    /// them, because two ideas of what a row is would disagree the moment one was fixed.
    ///
    /// The cost is stated rather than hidden: every read is the backend's full answer on the
    /// wire, about a thousand rows. That is a bounded, human-frequency request - a person or
    /// an agent asking what a pane has printed - rather than anything on the render path.
    #[must_use]
    pub fn tail(self, rows: u32) -> PaneText {
        let held = crate::find::rows_of(&self.text);
        let wanted = rows as usize;
        if rows == 0 || held.len() <= wanted {
            return self;
        }
        PaneText {
            text: held[held.len() - wanted..].join("\n") + "\n",
            // There is history this answer did not reach, which is what the flag has always
            // meant - now true because Muster dropped it rather than because herdr did.
            truncated: true,
        }
    }
}

#[cfg(test)]
mod pane_text_tests {
    use super::PaneText;

    fn read(text: &str, truncated: bool) -> PaneText {
        PaneText { text: text.to_string(), truncated }
    }

    /// The bug this exists for, in miniature: a caller asks for fewer rows than the pane holds
    /// and gets the newest ones, which is what "the last forty" has always meant to whoever
    /// typed it.
    #[test]
    fn a_count_takes_the_newest_rows() {
        let cut = read("one\ntwo\nthree\nfour\n", false).tail(2);
        assert_eq!(cut.text, "three\nfour\n");
        assert!(
            cut.truncated,
            "two rows were left above, which is history this read did not reach"
        );
    }

    /// Asking for more than there is is not an error and does not pad. It is also the case
    /// that must not set `truncated`: a caller told there is more when there is not stops
    /// believing the flag on the reads where it matters.
    #[test]
    fn asking_for_more_than_there_is_answers_with_what_there_is() {
        let whole = read("one\ntwo\n", false).tail(50);
        assert_eq!(whole.text, "one\ntwo\n");
        assert!(!whole.truncated);
    }

    /// Zero is "as far back as you have", which is what `pane read` with no `--rows` means.
    #[test]
    fn zero_is_everything() {
        assert_eq!(read("one\ntwo\n", false).tail(0).text, "one\ntwo\n");
    }

    /// A backend that truncated says so whatever the count does. The two are different
    /// claims about the same sentence - one is what herdr could not reach, the other what
    /// Muster chose not to hand over - and either one makes it true.
    #[test]
    fn a_backends_own_truncation_survives_a_count_that_cut_nothing() {
        assert!(read("one\ntwo\n", true).tail(50).truncated);
        assert!(read("one\ntwo\n", true).tail(0).truncated);
    }
}

/// Where a pane is looking, and how much of it is on screen.
///
/// Asked for rather than followed. A backend reports this on every pane payload, and Muster
/// does not keep it: the one topic that announces a change to it needs a subscription per
/// pane, so a held copy would be right until somebody touched the wheel. Nothing renders
/// from this - it exists so that landing on something found deep in a pane's history can be
/// worked out, and that is worth one round trip at the moment somebody asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Viewport {
    /// How far above the bottom of the history the lowest visible row sits. Zero is a pane
    /// showing its newest output, which is where a pane nobody has scrolled always is.
    pub rows_from_bottom: u32,
    /// How many rows are on screen.
    pub rows: u32,
    /// The furthest up this pane can go, which is zero until something has scrolled off it.
    pub deepest: u32,
}

impl Viewport {
    /// Whether a row is already on screen.
    ///
    /// The lowest visible row is [`Viewport::rows_from_bottom`] and the screen climbs from
    /// there, so a viewport at 100 showing 24 rows covers 100 to 123.
    pub fn shows(&self, rows_from_bottom: u32) -> bool {
        rows_from_bottom >= self.rows_from_bottom
            && rows_from_bottom < self.rows_from_bottom.saturating_add(self.rows)
    }

    /// Where the viewport would have to be for a row to sit in the middle of it.
    ///
    /// The middle rather than an edge, because a match at the bottom of a screen is a match
    /// whose consequences are off it - and the line after an error is usually the reason.
    /// Clamped to what the pane holds, so asking for a row near either end lands against
    /// that end rather than nowhere.
    pub fn centred_on(&self, rows_from_bottom: u32) -> u32 {
        rows_from_bottom.saturating_sub(self.rows.saturating_sub(1) / 2).min(self.deepest)
    }

    /// How many rows the pane holds in all, screen included.
    ///
    /// [`Viewport::deepest`] is how far the screen can climb, so the pane is that plus one
    /// screenful. This is what says whether a read reached everything, and what a search
    /// reports when it did not.
    pub fn rows_held(&self) -> u32 {
        self.deepest.saturating_add(self.rows)
    }
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

impl Snapshot {
    /// The tabs this snapshot lists that none of its own panes are in.
    ///
    /// A snapshot disagreeing with itself, and the answer to it is settled: **a tab is never
    /// legitimately empty**, because on creation the pane arrives before the tab
    /// (`observations/herdr-0.8.0.md` section 15, measured on the events). So "this tab holds
    /// no panes" means one thing, and `Mirror::remove_pane` already acts on it - it drops a
    /// tab whose last pane went, because herdr closes such a tab and announces nothing.
    ///
    /// Bootstrap needs the same rule, and not having it is what put five tabs in a window
    /// holding one (kan a_2HvxMgXai). A daemon there answered `session.snapshot` with tabs its
    /// own `tab list` denied holding, and Muster drew every one: rows taking chord numbers, a
    /// region on a tab with no pane to close, and an agent list four fifths fiction.
    ///
    /// Shared with the adapter so it can say how many were denied without restating the rule.
    pub fn empty_tabs(&self) -> Vec<TabId> {
        let occupied: BTreeSet<&TabId> = self.panes.iter().map(|pane| &pane.tab).collect();
        self.tabs
            .iter()
            .filter(|tab| !occupied.contains(&tab.id))
            .map(|tab| tab.id.clone())
            .collect()
    }
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
