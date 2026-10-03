//! What Muster asks a backend to change.
//!
//! Muster never mutates: it requests, and what it holds afterwards is whatever the daemon
//! said rather than what the request hoped for (`docs/architecture.md`, ownership of truth).
//! An answer is one of the two ways a daemon says something - a statement about a change it
//! has just made, arriving on the request channel instead of the event stream - so what comes
//! back here may be applied, and nothing here may be assumed.
//!
//! Named for what a view wants rather than for what a daemon offers, like every other noun
//! Muster owns: a window asks for a side, because that is the question a person answered when
//! they pressed the key.

use crate::mirror::backend::{PaneId, TabId};
use crate::pane_text::PaneText;

/// A direction on screen, as a person means it.
///
/// Muster's own word rather than the daemon's, on the same terms as `SplitAxis`.
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

/// Where a pane being moved is going.
///
/// Three rather than one because a person means three different things. "Put this beside that"
/// is what dragging a row onto another row is, and it needs somewhere that already exists to
/// land. "Give this a tab of its own" names nowhere, and used to cost three commands and a login
/// shell started and killed on the way (kan `a_2IXGSgZi7`). "Put this in that tab" names a tab
/// and nothing inside it, which is the one that can span machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveDestination {
    /// Beside a pane, in that pane's tab, on one side of it.
    ///
    /// Two callers mean two things by it. Dropping a pane on another pane's edge names the side
    /// outright. Dropping a row on a row in another tab means "behind this one in the list",
    /// which is `Side::Right`: the list reads the tab's panes in that order, so the next place
    /// in it is to the right.
    Beside { tab: TabId, pane: PaneId, side: Side },

    /// Into a tab of its own, which the move makes.
    ///
    /// One request rather than three, and that is the whole of why it exists: making a tab and
    /// then moving into it starts a pane nobody wanted, moves the keyboard into it for as long
    /// as it lives, and leaves a stray tab behind if anything fails in between.
    ///
    /// `name` is what to call the new tab, or nothing to leave it unnamed. The dance this
    /// replaces could not name it at all.
    NewTab { tab: TabId, name: Option<String> },

    /// Into a Muster tab that exists, wherever in it this pane's machine lands.
    ///
    /// **The one destination that can cross machines**, and the whole of what makes a tab hold a
    /// laptop pane beside a devenv pane (MIP-2, stage four). The pane does not move machines -
    /// it is a process and stays where it is - what moves is which Muster tab it belongs to, and
    /// a tab is a grouping Muster made rather than anything a daemon holds.
    ///
    /// The adapter does the work in two shapes. A tab that already has a part on this pane's
    /// machine takes the pane into it, beside that part's last pane. A tab that does not gets
    /// one: the pane goes into a new tab there under the same Muster name, which is what makes
    /// the two machines' parts one tab (MIP-3, section 2).
    ///
    /// Distinct from [`MoveDestination::Beside`] rather than a second meaning for its `tab`,
    /// because the two are different requests: `Beside` orders one pane against another and both
    /// have to be in one tree, which is one machine's. This names no pane and orders nothing.
    Tab { tab: TabId },
}

/// Which child a step down a tree takes.
///
/// A tree is addressed by the turns taken to reach a node, because the nodes have no names -
/// a divider is not a thing a daemon hands out an id for, it is a position in a shape that
/// changes under it. Turns stay meaningful as the tree around them changes shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    First,
    Second,
}

/// One requested change.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendIntent {
    /// Splits a pane, putting the new one on the named side of it. The adapter mints the new
    /// pane's name and says it in [`Outcome::created`].
    SplitPane {
        pane: PaneId,
        side: Side,
        /// The existing pane's share afterwards. `None` takes the daemon's own default,
        /// which is what a keybinding wants; a drag-to-split would say.
        ///
        /// The existing pane's rather than the first child's, so that one number means one
        /// thing on all four sides.
        ratio: Option<f32>,
        /// Where the new pane starts. `None` takes the daemon's own rule, the directory the
        /// split came from - what somebody splitting a pane mid-task means, and the reason
        /// this is not resolved here.
        cwd: Option<String>,
        /// What to run in it, as somebody would have typed it. `None` runs whatever a new pane
        /// runs anyway.
        ///
        /// Part of making the pane, not a thing to do afterwards: the daemon starts it with the
        /// pane, so no caller races a shell prompt it cannot see.
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
    /// Makes a tab, with one pane of its own in it. The adapter mints the pane's name and says
    /// it in [`Outcome::created`].
    ///
    /// The one intent that needs nothing to exist, so it is also what is asked for when there
    /// is nothing: a daemon Muster just started holds no panes, and a window showing none of
    /// them is not a window.
    CreateTab {
        /// What to call the tab, which the window mints and takes before it asks: the daemon
        /// announces the tab before it answers, and a window that did not already hold it could
        /// lose it to another window in that moment.
        tab: TabId,
        /// Where its pane starts. Unlike a split, this is resolved before it is sent - a new
        /// tab has nothing to inherit from, and the daemon's own answer is a home directory
        /// nobody asked for.
        cwd: Option<String>,
        /// What to run in the tab's pane, and what to call it. Both mean what they mean on
        /// [`BackendIntent::SplitPane`], because a tab is the other way to make a pane and a
        /// caller equipping one and not the other would have to race a shell prompt itself.
        run: Option<String>,
        name: Option<String>,
    },
    /// Makes a pane on this machine as a new part of a Muster tab another machine already holds
    /// part of, which is what splitting a laptop pane onto a devenv means when the devenv has no
    /// part of that tab yet. The adapter mints the pane's name and says it in
    /// [`Outcome::created`].
    ///
    /// Not [`BackendIntent::CreateTab`], although the daemon is asked for the same placement:
    /// that makes a tab under a name the window has only just minted, and this joins one the
    /// window already holds. The window takes the one before asking and surfaces it after; doing
    /// either to a tab it already shows would be a write to the shared record of which window
    /// holds which tab, about a tab whose holder has not changed.
    JoinTab {
        tab: TabId,
        /// Where its pane starts. `None` takes the daemon's own default: the pane it is joining
        /// is on another machine, so its directory names nothing here.
        cwd: Option<String>,
        /// Both mean what they mean on [`BackendIntent::SplitPane`].
        run: Option<String>,
        name: Option<String>,
    },
    /// Grows or shrinks a pane against its neighbour, by a share of the region.
    ///
    /// Unlike `SetSplitRatio`, which names a divider by the turns down to it and says exactly
    /// where it should sit, this names a pane and a direction. That is what a keystroke means:
    /// somebody holding a chord down wants this pane bigger, and which divider moves to
    /// achieve that is a question about a tree they are not looking at.
    ///
    /// The daemon resolves which divider a direction refers to.
    ResizePane {
        pane: PaneId,
        direction: Side,
        /// How far, as a share of the region between 0 and 1. `None` takes the daemon's own
        /// step, which is what a keybinding wants.
        ///
        /// A fraction rather than a distance, and named for it, because what moves is a
        /// divider's ratio and the daemon has no idea how many points a cell is.
        fraction: Option<f32>,
    },

    /// Makes one pane fill its tab, or puts it back.
    ///
    /// A toggle, because that is what one key does. The adapter reads what is zoomed now from
    /// its mirror and asks the daemon for the other.
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
    /// Both panes are in the same tab. Crossing tabs is [`BackendIntent::MovePane`].
    SwapPanes {
        pane: PaneId,
        with: PaneId,
    },

    /// Moves a pane out of its tab and puts it somewhere else on the same daemon.
    ///
    /// Same daemon only, and that is not a limitation worth working around: a pane is a PTY the
    /// daemon owns, so "move it to the other machine" would mean killing a process on one host
    /// and starting a different one on another. That is not a move, and the shell refuses the
    /// drop rather than sending this.
    MovePane {
        pane: PaneId,
        to: MoveDestination,
    },

    /// Closes a tab, and every pane in it with it.
    ///
    /// The one verb here that destroys more than it names. Muster sends it rather than closing
    /// the panes one at a time because a daemon closing its own tab is one act, where N closes
    /// is N chances to be interrupted half way and left with a tab holding one pane.
    CloseTab {
        tab: TabId,
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
    /// The name is the daemon's to keep, which is the whole reason this is an intent rather
    /// than something Muster remembers: any client can set one, the daemon writes it down, and
    /// it comes back after a daemon restart. Muster holding its own would be a second answer
    /// that no other client could see and that a restart would strand.
    RenamePane {
        pane: PaneId,
        /// `None` takes the name away, leaving the pane called after its directory again.
        name: Option<String>,
    },

    /// Calls a tab what somebody wants to call it.
    ///
    /// Separate from [`BackendIntent::RenamePane`] rather than one verb over a target, because
    /// a tab's name is held on every machine the tab spans and is ordered by a generation.
    RenameTab {
        tab: TabId,
        /// `None` takes the name away, leaving the tab numbered again.
        name: Option<String>,
        /// Higher than any this tab's name has had on any machine, so that every part adopts
        /// it and a part that missed it is renamed when its daemon reconnects (MIP-3, section
        /// 2). The daemon refuses a lower one.
        generation: u64,
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
            BackendIntent::RenameTab { tab, name, generation } => format!(
                "RenameTab {{ tab: {tab}, name: {}, generation: {generation} }}",
                named(name.as_deref())
            ),
            // A command line, for the reason above and one more: an environment set on the way
            // to a program is a normal thing to type, and a token is a normal thing to set.
            BackendIntent::SplitPane { pane, side, ratio, cwd, run, name } => format!(
                "SplitPane {{ pane: {pane}, side: {side:?}, ratio: {ratio:?}, cwd: {cwd:?}, \
                 run: {}, name: {} }}",
                counted(run.as_ref()),
                named(name.as_deref())
            ),
            BackendIntent::CreateTab { tab, cwd, run, name } => format!(
                "CreateTab {{ tab: {tab}, cwd: {cwd:?}, run: {}, name: {} }}",
                counted(run.as_ref()),
                named(name.as_deref())
            ),
            BackendIntent::JoinTab { tab, cwd, run, name } => format!(
                "JoinTab {{ tab: {tab}, cwd: {cwd:?}, run: {}, name: {} }}",
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

/// What a daemon said about a change it just made.
///
/// Only what no event can say: *which* of the things that appeared is the one this request
/// made, which Muster uses to point its keyboard. Everything the change did arrived as events
/// before the answer, so by the time a submit returns the mirror already shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The pane a request made, when it made one.
    pub created: Option<PaneId>,
    /// The tab a request made, when it made one.
    ///
    /// Needed for the same reason and by a different part of the window: a new tab is
    /// somewhere no region is looking, and Muster decides what a region shows itself
    /// (`architecture.md`, cursors are written, not read).
    pub created_tab: Option<TabId>,
}

/// Why a backend did not come back saying a change was made.
///
/// Mostly prose to hand back to whoever asked, because there is usually no second thing to
/// try: a refused split is a split that did not happen, and the honest response is to say so
/// rather than to answer as though it had. Two kinds are different, and are why this is not
/// just a string.
///
/// A request whose state already holds is a success, not one of these (`architecture.md`,
/// degradation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The window's picture of what the request named is stale.
    ///
    /// Not a failure of the request so much as a report about Muster: the window is showing
    /// something that is not there, and every later request about it will be refused the same
    /// way. The answer is to ask the daemon what it does hold.
    NotThere(String),

    /// The backend was asked and never said what came of it.
    ///
    /// Not a refusal at all, and the one answer here that must not be reported as one: the
    /// request may have reached the daemon, so the change may well have happened and only the
    /// answer was lost - a caller told it was refused sends the request again (kan
    /// a_2LOHfLmsL). Whatever did happen arrives on the daemon's own events.
    Unanswered(String),

    /// Anything else. The request did not happen, and saying so is all there is to do.
    Declined(String),
}

impl Refusal {
    /// What the backend said, for a log or a message back to whoever asked.
    pub fn detail(&self) -> &str {
        match self {
            Refusal::NotThere(detail) | Refusal::Unanswered(detail) | Refusal::Declined(detail) => {
                detail
            }
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

    /// Reads a pane's history back, and changes nothing: its last `rows` rows, or as far back as
    /// the daemon holds for zero.
    ///
    /// A backend may hand back more than was asked for, as a daemon that cannot read from the
    /// end does, so how much of it a caller wanted is still [`PaneText::tail`], after. A pane's
    /// output never enters the core, so this is the only way anything above the seam sees what a
    /// pane has printed.
    ///
    /// A read rather than an intent because nothing changes: `BackendIntent` is what Muster
    /// asks a daemon to *do*, and putting a question in it would make `Outcome` - a statement
    /// about a change just made - carry answers to things that changed nothing.
    fn read(&self, pane: &PaneId, rows: u32) -> Result<PaneText, Refusal>;

    /// What this channel is talking to, for the log.
    fn description(&self) -> &str;
}
