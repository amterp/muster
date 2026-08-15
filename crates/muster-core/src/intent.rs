//! What Muster asks a backend to change.
//!
//! Muster never mutates: it requests, and finds out what happened by watching the events
//! that follow (`docs/architecture.md`, the vocabulary). Nothing here returns new state, and
//! nothing above here may assume a request took effect - the mirror says what is true, and
//! it says so when the daemon does.
//!
//! Named for what a view wants rather than for what a backend offers, like every other noun
//! Muster owns. herdr spells a split as the direction the new pane went; a window asks for
//! a column or a row, because that is the question a person answered when they pressed the
//! key.

use crate::mirror::backend::{PaneId, SplitAxis, TabId, WorkspaceId};

/// Which child a step down a tree takes.
///
/// A tree is addressed by the turns taken to reach a node, because the nodes have no names -
/// a divider is not a thing a backend hands out an id for, it is a position in a shape that
/// changes under it. Turns stay meaningful as the tree around them changes shape, and they
/// are what the reconstruction in the adapter already produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    First,
    Second,
}

/// One requested change.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendIntent {
    /// Splits a pane, putting the new one beside or below it.
    SplitPane {
        pane: PaneId,
        axis: SplitAxis,
        /// The existing pane's share afterwards. `None` takes the backend's own default,
        /// which is what a keybinding wants; a drag-to-split would say.
        ratio: Option<f32>,
        /// Where the new pane starts. `None` takes the backend's own rule, which for herdr
        /// means the directory the split came from - what somebody splitting a pane mid-task
        /// means, and the reason this is not resolved here.
        cwd: Option<String>,
    },
    ClosePane {
        pane: PaneId,
    },
    /// Moves the *backend's* focus, which is not the same as moving Muster's keyboard.
    ///
    /// Muster routes input by its own view-local cursor and writes this as a side effect, so
    /// that a daemon computing seen-ness has been told somebody looked (`architecture.md`,
    /// cursors are written, not read).
    FocusPane {
        pane: PaneId,
    },
    /// Makes a tab in a workspace, with one pane in it.
    ///
    /// A tab rather than a workspace, because a workspace is herdr's unit for a whole project
    /// and a tab is the unit somebody reaches for several times an hour.
    ///
    /// The workspace is named outright, unlike every other verb here, which names a pane. It
    /// has to be: herdr's `tab.create` takes a workspace and nothing else, and it ignores
    /// keys it does not know - so a pane id sent hopefully would be dropped in silence and
    /// the tab would appear in whichever workspace that daemon happened to have focused
    /// (`observations/herdr-0.8.0.md` section 6). Which workspace a pane is in is the
    /// mirror's answer, and it is given before this is built.
    CreateTab {
        workspace: WorkspaceId,
        /// Where its pane starts. Unlike a split, this is resolved before it is sent - a new
        /// tab has nothing to inherit from, and the backend's own answer is a home directory
        /// nobody asked for.
        cwd: Option<String>,
    },
    /// Makes a workspace, with one tab and one pane in it.
    ///
    /// The only intent that names nothing existing, because it is the one asked for when
    /// there is nothing: a daemon Muster just started holds no panes, and a window showing
    /// none of them is not a window. Every other verb here needs a pane to point at.
    CreateWorkspace {
        /// Where its first pane starts. `None` takes the daemon's own default rather than
        /// this process's directory - Muster's cwd is wherever the app was launched from,
        /// which is meaningless to whoever is looking at the window.
        cwd: Option<String>,
    },
    /// Moves one divider in a tab's tree.
    SetSplitRatio {
        tab: TabId,
        /// The turns from the tab's root to the split being moved.
        path: Vec<Branch>,
        /// The first child's share afterwards, between 0 and 1.
        ratio: f32,
    },
}

/// What a backend said about a change it just made.
///
/// Not state, and not a shortcut around the event stream: what the session now looks like
/// still arrives on the daemon's own events, and the mirror still learns it there. What is
/// here is the one thing those events cannot answer - *which* of the panes that appeared is
/// the one this request created - and Muster needs it only to point its own keyboard, which
/// is Muster's state rather than the daemon's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The pane a split made, when the request made one.
    pub created: Option<PaneId>,
    /// The tab a request made, when it made one.
    ///
    /// Needed for the same reason and by a different part of the window: a new tab is
    /// somewhere no region is looking, and Muster decides what a region shows without ever
    /// reading the daemon's own focus (`architecture.md`, cursors are written, not read). So
    /// the answer has to come back with the request that caused it.
    pub created_tab: Option<TabId>,
}

/// A way to ask one backend for a change.
///
/// One per daemon rather than one per pane, unlike the input channels: these are about
/// structure, and structure belongs to the daemon rather than to any pane in it.
pub trait BackendChannel: Send + Sync + std::fmt::Debug {
    /// Asks, and says why not.
    ///
    /// The error is prose for a log rather than a code to branch on, because there is no
    /// second thing to try: a refused split is a split that did not happen, and the honest
    /// response is to say so where somebody will read it.
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, String>;

    /// What this channel is talking to, for the log.
    fn description(&self) -> &str;
}
