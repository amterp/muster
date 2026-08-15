//! What Muster asks a backend to change.
//!
//! Muster never mutates: it requests, and what it holds afterwards is whatever the daemon
//! said rather than what the request hoped for (`docs/architecture.md`, ownership of truth).
//! An answer is one of the two ways a daemon says something - a statement about a change it
//! has just made, arriving on the request channel instead of the event stream - so what comes
//! back here may be applied, and nothing here may be assumed.
//!
//! Named for what a view wants rather than for what a backend offers, like every other noun
//! Muster owns. herdr spells a split as the direction the new pane went; a window asks for a
//! side, because that is the question a person answered when they pressed the key.

use crate::mirror::backend::{Layout, PaneId, TabId, WorkspaceId};

/// A direction on screen, as a person means it.
///
/// Muster's own word rather than a backend's, on the same terms as `SplitAxis`: herdr spells
/// these the same way today, and a second backend spelling them `west` costs one match arm in
/// its adapter rather than a rename through the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    /// The name a chord, a menu item and a CLI all spell it with.
    pub fn parse(name: &str) -> Option<Side> {
        match name {
            "left" => Some(Side::Left),
            "right" => Some(Side::Right),
            "up" => Some(Side::Up),
            "down" => Some(Side::Down),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Up => "up",
            Side::Down => "down",
        }
    }
}

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
    /// Splits a pane, putting the new one on the named side of it.
    ///
    /// All four sides, which is not what every backend offers: herdr places a new pane on the
    /// `second` side and has only `right` and `down`, so two of these are a split and a swap
    /// rather than one request. That is the adapter's problem, deliberately - the question a
    /// person answered when they pressed the key was "which side", and a core that only had
    /// two of the four answers would be a core shaped by one daemon's spelling.
    SplitPane {
        pane: PaneId,
        side: Side,
        /// The existing pane's share afterwards. `None` takes the backend's own default,
        /// which is what a keybinding wants; a drag-to-split would say.
        ///
        /// The existing pane's rather than the first child's, so that one number means one
        /// thing on all four sides. An adapter whose backend counts from the other end
        /// inverts it.
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
    /// Grows or shrinks a pane against its neighbour, in cells.
    ///
    /// Unlike `SetSplitRatio`, which names a divider by the turns down to it and says exactly
    /// where it should sit, this names a pane and a direction. That is what a keystroke means:
    /// somebody holding a chord down wants this pane bigger, and which divider moves to
    /// achieve that is a question about a tree they are not looking at.
    ///
    /// The backend resolves it, and it is the only verb here that could not be built from the
    /// mirror - deciding which divider a direction refers to needs the rects, which are the
    /// daemon's own and change under a viewport this window does not control.
    ResizePane {
        pane: PaneId,
        direction: Side,
        /// How much, in cells. `None` takes the backend's own step, which is what a
        /// keybinding wants.
        amount: Option<f32>,
    },

    /// Makes one pane fill its tab, or puts it back.
    ///
    /// A toggle rather than a state, because that is what one key does. What is zoomed is
    /// daemon truth and arrives on the mirror; asking for `on` or `off` would mean reading it
    /// back first, which is a round trip to answer a question the daemon is about to answer
    /// anyway.
    ZoomPane {
        pane: PaneId,
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

/// An arrangement a daemon stated in its answer, and what that answer left behind.
///
/// Daemon truth on the same terms as an event, and the reason it is worth taking here is
/// timing: herdr answers a swap with the settled tree in about a millisecond and broadcasts
/// the same tree about a hundred milliseconds later
/// (`observations/herdr-0.8.0.md` section 14). A window that waits to be told twice is a
/// window that renders the arrangement it was moving away from.
#[derive(Debug, Clone, PartialEq)]
pub struct SettledLayout {
    /// How the tab is arranged now, as the daemon said when it was asked.
    pub layout: Layout,
    /// An arrangement the daemon published on its way here, which has not reached the
    /// subscription yet and is already out of date when it does.
    ///
    /// Only a backend that needs two requests for one intent has one of these, and only its
    /// adapter can know what it looked like. `None` everywhere else, including the case the
    /// mirror handles for itself: what a tab was arranged as *before* this answer is
    /// something the mirror is already holding, and does not have to be told.
    pub stale: Option<Layout>,
}

/// What a backend said about a change it just made.
///
/// Two kinds of thing, and the difference is who they are for. `created` and `created_tab`
/// answer what no event can - *which* of the things that appeared is the one this request
/// made - and are Muster's own state, used to point its keyboard. `settled` is daemon truth,
/// and is here because a daemon answers faster than it broadcasts.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// How a tab is arranged now, when the daemon's answer said.
    pub settled: Option<SettledLayout>,
}

/// Why a backend would not make a change.
///
/// Mostly prose for a log, because there is usually no second thing to try: a refused split
/// is a split that did not happen, and the honest response is to say so where somebody will
/// read it. One kind is different, and is why this is not just a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The backend does not hold what the request named.
    ///
    /// Not a failure of the request so much as a report about Muster: the window is showing
    /// something that is not there, and every later request about it will be refused the same
    /// way. A daemon can drop a pane without saying so - herdr does, when a pane's terminal
    /// goes - so this is sometimes the only account of it there is, and it is worth acting on
    /// rather than logging.
    NotThere(String),

    /// Anything else. Worth a log line and nothing more.
    Declined(String),
}

impl Refusal {
    /// What the backend said, for a log or a message back to whoever asked.
    pub fn detail(&self) -> &str {
        match self {
            Refusal::NotThere(detail) | Refusal::Declined(detail) => detail,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.detail())
    }
}

/// A way to ask one backend for a change.
///
/// One per daemon rather than one per pane, unlike the input channels: these are about
/// structure, and structure belongs to the daemon rather than to any pane in it.
pub trait BackendChannel: Send + Sync + std::fmt::Debug {
    /// Asks, and says why not.
    fn submit(&self, intent: &BackendIntent) -> Result<Outcome, Refusal>;

    /// What this channel is talking to, for the log.
    fn description(&self) -> &str;
}
