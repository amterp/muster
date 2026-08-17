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

use crate::find::{Found, Needle};
use crate::mirror::backend::{Layout, PaneId, TabId, Viewport, WorkspaceId};

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
        /// What to run in it, as somebody would have typed it. `None` runs whatever a new pane
        /// runs anyway.
        ///
        /// Part of making the pane, not a thing to do afterwards, and that is a decision about
        /// where a cost belongs rather than a convenience. A backend that can spawn a program
        /// with a pane does this in one request; herdr cannot, so its adapter waits for the
        /// pane's shell to draw a prompt and then types. Either way one caller asked for a pane
        /// running something and got one, instead of every caller racing a prompt it cannot
        /// see.
        run: Option<String>,
        /// What to call it. `None` leaves it unnamed.
        ///
        /// Along with the split because they are one intention: an agent making three panes has
        /// to be able to say which is which, and a rename arriving separately would leave the
        /// pane briefly nameless in every window showing it.
        name: Option<String>,
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
    /// Types into a pane, whether or not anything is showing it.
    ///
    /// Not the keyboard. [`PaneInput`](crate::input::PaneInput) encodes a keystroke against the
    /// live modes of the pane this window's keyboard feeds and writes it down that pane's own
    /// channel; this goes out through the daemon and names the pane, so it reaches one in a tab
    /// nobody is looking at. An agent instructing another agent needs exactly that, and no
    /// keystroke can do it.
    SendText {
        pane: PaneId,
        text: String,
        /// Whether to press Return afterwards.
        ///
        /// Its own flag rather than a newline in the text. Once a program is reading, the two
        /// are different things: Return is encoded against the pane's modes, and a bare newline
        /// inside a bracketed paste is text rather than a submission. A harness that reads one
        /// and not the other is the common case.
        enter: bool,
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
    /// Grows or shrinks a pane against its neighbour, by a share of the region.
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
        /// How far, as a share of the region between 0 and 1. `None` takes the backend's own
        /// step, which is what a keybinding wants.
        ///
        /// A fraction rather than a distance, and named for it, because a distance is not
        /// something the far side of this seam can act on: what moves is a divider's ratio,
        /// and the backend has no idea how many points a cell is. This field said "cells" for
        /// one release and the backend read it as a fraction the whole time, which made every
        /// step a person could write land on the same maximal jump - a disagreement two doc
        /// comments could hold at once precisely because `amount` named neither.
        fraction: Option<f32>,
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

    /// Exchanges two panes' places in their tab's tree.
    ///
    /// What dragging one row past another in the agent list means. An exchange rather than an
    /// insertion because a tree has no "between": moving a pane to an arbitrary position would
    /// mean rebuilding the arrangement around it, and there is no reading of that a person
    /// dragging one row expects.
    ///
    /// Both panes are in the same tab. Crossing tabs is [`BackendIntent::MovePane`], which is a
    /// different request to the backend and answers with two arrangements rather than one.
    ///
    /// The adapter already issues its backend's swap as the invisible second half of a leftward
    /// split; this is the same request asked for on its own.
    SwapPanes {
        pane: PaneId,
        with: PaneId,
    },

    /// Moves a pane out of its tab and into another one on the same daemon.
    ///
    /// `after` is the pane in the destination it lands behind, in the order the tab lays its
    /// panes out - which is the order the agent list reads. An ordering rather than a side,
    /// because that is what dragging a row down a list means; the adapter spells it in whatever
    /// geometry its backend has.
    ///
    /// Same daemon only, and that is not a limitation worth working around: a pane is a PTY the
    /// daemon owns, so "move it to the other machine" would mean killing a process on one host
    /// and starting a different one on another. That is not a move, and the shell refuses the
    /// drop rather than sending this.
    MovePane {
        pane: PaneId,
        tab: TabId,
        after: PaneId,
    },

    /// Moves one divider in a tab's tree.
    SetSplitRatio {
        tab: TabId,
        /// The turns from the tab's root to the split being moved.
        path: Vec<Branch>,
        /// The first child's share afterwards, between 0 and 1.
        ratio: f32,
    },

    /// Calls a pane what somebody wants to call it.
    ///
    /// The name is the backend's to keep, which is the whole reason this is an intent rather
    /// than something Muster remembers: any client can set one, herdr writes it down, and it
    /// comes back after a daemon restart. Muster holding its own would be a second answer that
    /// no other client could see and that a restart would strand.
    RenamePane {
        pane: PaneId,
        /// `None` takes the name away, leaving the pane called after its directory again.
        name: Option<String>,
    },

    /// Calls a tab what somebody wants to call it.
    ///
    /// Separate from [`BackendIntent::RenamePane`] rather than one verb over a target, because
    /// the two are not the same operation underneath: herdr announces a tab rename and says
    /// nothing at all about a pane one, and only the pane's can be undone.
    RenameTab {
        tab: TabId,
        /// `None` asks for the name to be taken away, which no backend has to be able to do
        /// completely. herdr cannot: its `tab.rename` takes a required string, so the adapter
        /// sends an empty one and the tab is left nameless rather than renumbered
        /// (`observations/herdr-0.8.0.md` section 16). The intent says what was asked for; how
        /// far a backend can honour it is the adapter's to report.
        name: Option<String>,
    },
}

impl BackendIntent {
    /// This intent as a log line: everything about it except anything somebody typed.
    ///
    /// A name is text a person wrote about their own work - "🔥 payments spike" says what they
    /// are doing and possibly who for - and the run log is a file destined for a bug report.
    /// The same rule keystrokes already follow: what was pressed is recorded by shape rather
    /// than by content, and what a name says is recorded as whether there was one
    /// (`architecture.md`, the diagnostic log).
    ///
    /// Everything else is its ordinary debug form, because a split's side and a resize's step
    /// are facts about Muster rather than about the person using it.
    pub fn redacted(&self) -> String {
        match self {
            BackendIntent::RenamePane { pane, name } => {
                format!("RenamePane {{ pane: {pane}, name: {} }}", named(name.as_deref()))
            }
            BackendIntent::RenameTab { tab, name } => {
                format!("RenameTab {{ tab: {tab}, name: {} }}", named(name.as_deref()))
            }
            // What somebody types into their own terminal, on the same terms as a find needle
            // and a pane's name: the length says whether it arrived and how much of it, which
            // is what a log is read for, and the words are theirs.
            BackendIntent::SendText { pane, text, enter } => {
                format!(
                    "SendText {{ pane: {pane}, text: {}, enter: {enter} }}",
                    counted(Some(text))
                )
            }
            // A command line, for the reason above and one more: an environment set on the way
            // to a program is a normal thing to type, and a token is a normal thing to set.
            BackendIntent::SplitPane { pane, side, ratio, cwd, run, name } => format!(
                "SplitPane {{ pane: {pane}, side: {side:?}, ratio: {ratio:?}, cwd: {cwd:?}, \
                 run: {}, name: {} }}",
                counted(run.as_ref()),
                named(name.as_deref())
            ),
            other => format!("{other:?}"),
        }
    }
}

/// How much text there was, without saying what it said.
fn counted(text: Option<&String>) -> String {
    match text {
        Some(text) => format!("<{} character(s)>", text.chars().count()),
        None => "<none>".to_string(),
    }
}

/// Whether a rename asked for a name or asked for none, without saying what it was.
fn named(name: Option<&str>) -> &'static str {
    match name {
        Some(_) => "<given>",
        None => "<cleared>",
    }
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
    /// What a pane is called now, when the request renamed one.
    ///
    /// The only route there is. A backend need not announce a rename, and herdr does not:
    /// there is no event for one and no counter that moves, so a reply is the whole of what
    /// a client learns (`observations/herdr-0.8.0.md` section 16). Without taking it here,
    /// naming a pane changes the daemon and not the window, and stays that way until the
    /// connection next re-snapshots.
    ///
    /// The inner `Option` is the name itself, absent when the rename took one away, so that
    /// "no rename happened" and "the name is now nothing" are different answers.
    pub renamed: Option<(PaneId, Option<String>)>,
}

/// Why a backend would not make a change.
///
/// Mostly prose to hand back to whoever asked, because there is usually no second thing to
/// try: a refused split is a split that did not happen, and the honest response is to say so
/// rather than to answer as though it had. One kind is different, and is why this is not just
/// a string.
///
/// A backend that answers a change it declined with an ordinary success has to produce one of
/// these itself, which is a reading of its own vocabulary: a request whose state already holds
/// is a success, and one that did not happen is a refusal. herdr needs it - see
/// `muster_herdr::considered` - and `architecture.md` states the rule under degradation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The window's picture of what the request named is stale.
    ///
    /// Not a failure of the request so much as a report about Muster: the window is showing
    /// something that is not there, and every later request about it will be refused the same
    /// way. A daemon can drop a pane without saying so - herdr does, when a pane's terminal
    /// goes - so this is sometimes the only account of it there is, and it is worth acting on
    /// rather than logging.
    ///
    /// Usually a thing the backend does not hold at all. It also covers a thing it holds
    /// somewhere else, when the refusal proves that much: Muster picks between swapping two
    /// panes and moving one by reading which tabs they are in, so a backend refusing that
    /// choice has said the window has a pane in the wrong tab. Either way the answer is to ask
    /// what it does hold, because it will not volunteer it.
    NotThere(String),

    /// Anything else. The request did not happen, and saying so is all there is to do.
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

    /// Looks for text in a pane, and changes nothing.
    ///
    /// The one read here, and the one place find is swappable. A backend that searches its
    /// own history answers this directly; one that does not reads the history back and
    /// matches it with `find::hits_in`, which is what herdr's adapter does today. Either
    /// way the answer is the same shape and everything above it is the same code, so
    /// gaining a daemon-side search is one function body rather than a feature rewritten.
    ///
    /// A read rather than an intent because nothing changes: `BackendIntent` is what Muster
    /// asks a backend to *do*, and putting a question in it would make `Outcome` - a
    /// statement about a change just made - carry answers to things that changed nothing.
    fn find(&self, pane: &PaneId, needle: &Needle) -> Result<Found, Refusal>;

    /// Where a pane is looking, so that something found can be scrolled to.
    ///
    /// Asked at the moment it is needed rather than followed, because the change to it is
    /// announced only to a subscription that names the pane - fifteen panes would be fifteen
    /// held connections for a number nothing renders. A wheel touched between this answer
    /// and the scroll that follows it makes the landing approximate, which is the honest
    /// cost of a backend that scrolls by steps rather than to a place.
    fn viewport(&self, pane: &PaneId) -> Result<Viewport, Refusal>;

    /// What this channel is talking to, for the log.
    fn description(&self) -> &str;
}
