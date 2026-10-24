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

use std::collections::BTreeMap;

use crate::mirror::backend::{PaneId, TabId};
use crate::pane_text::{PaneText, Scope};

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
#[derive(Debug, Clone, PartialEq)]
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
    ///
    /// `hidden` makes a tab no window shows, which is what hiding a pane is: the pane goes on
    /// running in a tab of its own that is listed apart, and showing it again is a move into a
    /// new tab that is not hidden.
    NewTab { tab: TabId, name: Option<String>, hidden: bool },

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
    /// because the two are different requests: `Beside` orders one pane against another in the
    /// tree of the machine running both. This names no pane and orders nothing.
    Tab { tab: TabId },

    /// Into a tab at the place a tree the window worked out over every machine the tab spans
    /// gives it: beside a pane on another machine, or as a column at the right of the whole tab
    /// (MIP-8, section 6).
    InTree(InTree),
}

/// A tab's whole tree as a pane being placed leaves it, written to the machine running that pane
/// (MIP-8, section 4). The tree names the pane as that machine's, so a pane being made is named
/// before it is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct InTree {
    pub tab: TabId,
    pub root: crate::mirror::backend::CopyNode,
    /// The tab's layout generation afterwards.
    pub generation: u64,
    /// The generation of the copy the tree was worked out from, 0 when this machine holds no
    /// part of the tab yet.
    pub based_on: u64,
    /// Whether the tab is hidden, which a part this makes has to say too: every part of a tab
    /// says the same.
    pub hidden: bool,
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

/// The pane a new pane is made as a helper of (MIP-7), and what that pane is called: the hidden
/// tab a maker's helpers share is named for it, and a maker on another machine is named by nothing
/// that machine's daemon holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maker {
    pub pane: PaneId,
    pub name: String,
}

/// One requested change.
/// Where a new tab's pane starts, and whether anybody chose it.
///
/// The difference decides what a directory that cannot be entered means. One somebody named is
/// what they wanted, so the tab is refused; one taken from the pane the tab was made beside was
/// only a guess at what they wanted, and the daemon starts the pane at home instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directory {
    Named(String),
    Inherited(String),
}

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
        /// Whether to make it quiet (`crate::mirror::Pane::quiet`). With the pane rather than
        /// after it for the reason `name` is: a pane quieted a moment late has already asked.
        quiet: bool,
        /// Whose helper it is, if anybody's. Never changed once the pane is made.
        maker: Option<Maker>,
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
        cwd: Option<Directory>,
        /// What to run in the tab's pane, and what to call it. Both mean what they mean on
        /// [`BackendIntent::SplitPane`], because a tab is the other way to make a pane and a
        /// caller equipping one and not the other would have to race a shell prompt itself.
        run: Option<String>,
        name: Option<String>,
        quiet: bool,
        /// A tab no window shows, so the window that asked does not take it: a pane made hidden
        /// from the start, which is an agent nobody is looking at.
        hidden: bool,
        /// Whose helper it is, as on [`BackendIntent::SplitPane`]. A hidden helper goes into the
        /// hidden tab its maker's helpers share rather than into `tab`, unless none holds one yet.
        maker: Option<Maker>,
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
        quiet: bool,
        /// Whether the tab being joined is hidden. Every part of a tab says the same, so a pane
        /// split from a hidden one onto another machine is hidden there too.
        hidden: bool,
        /// Whose helper it is, as on [`BackendIntent::SplitPane`].
        maker: Option<Maker>,
    },
    /// Makes `pane` on this machine at its leaf of a tab's whole tree, which the window worked
    /// out because the place is beside another machine's pane or at the edge of the whole tab
    /// (MIP-8, section 6). Makes this machine's part of the tab when it has none yet.
    ///
    /// Names its pane, unlike every other create: the tree has to say where the pane goes, so the
    /// window mints the name before asking.
    CreateInTree {
        pane: PaneId,
        at: InTree,
        /// Each means what it means on [`BackendIntent::JoinTab`].
        cwd: Option<String>,
        run: Option<String>,
        name: Option<String>,
        quiet: bool,
        maker: Option<Maker>,
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

    /// Moves a pane out of its place and puts it somewhere else.
    ///
    /// Asked of the daemon running the pane, which keeps running it: a pane is a PTY the daemon
    /// owns, so a move changes where it sits and never which machine it is on. Beside another
    /// machine's pane is a place in a tab's tree like any other ([`MoveDestination::InTree`]).
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

    /// Compacts the context of the agent in a pane, at its next idle, empty prompt.
    ///
    /// The daemon types it, because the daemon is what can tell when the agent's prompt is empty
    /// and what its harness calls compacting.
    CompactPane {
        pane: PaneId,
        /// What the compaction should keep, where the agent's harness takes one.
        focus: Option<String>,
        /// The prompt typed once the compaction has run, so the agent carries on.
        then: Option<String>,
    },

    /// Restarts the agent in a pane, in the session it reported: the daemon ends it and types the
    /// session's resume at the shell it leaves, the command a daemon restart would run.
    ///
    /// The daemon's to do, because it holds the terminal whose input has to be emptied first and
    /// knows how the agent resumes.
    RestartAgent {
        pane: PaneId,
        /// Set for the resumed agent alone.
        env: BTreeMap<String, String>,
    },

    /// Makes a pane quiet, or lets it ask for the person again.
    ///
    /// `None` is the other of what it is now, read from the mirror as zoom is, which is what a
    /// menu item means; `Some` is what a script means, which must not depend on what the pane
    /// was before.
    QuietPane {
        pane: PaneId,
        quiet: Option<bool>,
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

    /// Writes a tab's whole tree on one machine holding part of it, other machines' panes
    /// included: how a window catches a machine's copy up with the tab it merged from every
    /// copy (MIP-8, `crate::spanning`). The daemon refuses one worked out from an older copy
    /// than it holds, or one that would lose or invent a pane of its own.
    SetTabTree {
        tab: TabId,
        root: crate::mirror::backend::CopyNode,
        zoomed: Option<PaneId>,
        /// The tab's layout generation afterwards.
        generation: u64,
        /// The generation of the copy the tree was worked out from.
        based_on: u64,
    },

    /// Hides a tab from every window, or brings it back, its panes arranged as they were: how a
    /// maker's helpers come forward together and go back (MIP-7, section 5).
    SetTabHidden {
        tab: TabId,
        hidden: bool,
        /// A name the tab takes in the same change, at [`crate::making::HELPERS_TAB_GENERATION`]:
        /// a helpers' tab coming forward called after its maker as it is now.
        name: Option<String>,
    },

    /// Tells a daemon that a maker its helpers there name has closed on another machine, which
    /// only a window sees: they stop being quiet, and the person is told (MIP-7, section 6).
    MakerClosed {
        maker: PaneId,
        /// What the maker was called as it closed, for the person's notice.
        called: Option<String>,
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
            BackendIntent::SetTabHidden { tab, hidden, name } => format!(
                "SetTabHidden {{ tab: {tab}, hidden: {hidden}, name: {} }}",
                named(name.as_deref())
            ),
            BackendIntent::MakerClosed { maker, called } => {
                format!("MakerClosed {{ maker: {maker}, called: {} }}", named(called.as_deref()))
            }
            // A command line, for the reason above and one more: an environment set on the way
            // to a program is a normal thing to type, and a token is a normal thing to set.
            BackendIntent::SplitPane { pane, side, ratio, cwd, run, name, quiet, maker } => {
                format!(
                    "SplitPane {{ pane: {pane}, side: {side:?}, ratio: {ratio:?}, cwd: {cwd:?}, \
                 run: {}, name: {}, quiet: {quiet}, maker: {} }}",
                    counted(run.as_ref()),
                    named(name.as_deref()),
                    made_by(maker.as_ref())
                )
            }
            BackendIntent::CreateTab { tab, cwd, run, name, quiet, hidden, maker } => format!(
                "CreateTab {{ tab: {tab}, cwd: {cwd:?}, run: {}, name: {}, quiet: {quiet}, \
                 hidden: {hidden}, maker: {} }}",
                counted(run.as_ref()),
                named(name.as_deref()),
                made_by(maker.as_ref())
            ),
            BackendIntent::CreateInTree { pane, at, cwd, run, name, quiet, maker } => format!(
                "CreateInTree {{ pane: {pane}, tab: {}, generation: {}, based_on: {}, hidden: {}, \
                 cwd: {cwd:?}, run: {}, name: {}, quiet: {quiet}, maker: {} }}",
                at.tab,
                at.generation,
                at.based_on,
                at.hidden,
                counted(run.as_ref()),
                named(name.as_deref()),
                made_by(maker.as_ref())
            ),
            BackendIntent::JoinTab { tab, cwd, run, name, quiet, hidden, maker } => format!(
                "JoinTab {{ tab: {tab}, cwd: {cwd:?}, run: {}, name: {}, quiet: {quiet}, \
                 hidden: {hidden}, maker: {} }}",
                counted(run.as_ref()),
                named(name.as_deref()),
                made_by(maker.as_ref())
            ),
            BackendIntent::CompactPane { pane, focus, then } => format!(
                "CompactPane {{ pane: {pane}, focus: {}, then: {} }}",
                counted(focus.as_ref()),
                counted(then.as_ref())
            ),
            // A variable's value can be a proxy's password.
            BackendIntent::RestartAgent { pane, env } => format!(
                "RestartAgent {{ pane: {pane}, env: {:?} }}",
                env.keys().collect::<Vec<_>>()
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

/// Whose helper a new pane is, by the maker's id alone: its name is something somebody typed.
fn made_by(maker: Option<&Maker>) -> String {
    maker.map_or_else(|| "<none>".to_string(), |maker| maker.pane.to_string())
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
    /// The callsign its daemon drew that pane, when it said.
    pub callsign: Option<String>,
    /// The tab a request made, when it made one.
    ///
    /// Needed for the same reason and by a different part of the window: a new tab is
    /// somewhere no region is looking, and Muster decides what a region shows itself
    /// (`architecture.md`, cursors are written, not read).
    pub created_tab: Option<TabId>,
    /// What a restart did, which nothing on screen says once the agent is back.
    pub restarted: Option<Restarted>,
}

/// What a daemon did to restart a pane's agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restarted {
    /// The program it ended.
    pub program: String,
    /// It was killed, having not ended when asked to.
    pub killed: bool,
    /// The line typed at the pane's shell to resume the session.
    pub typed: String,
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
    ///
    /// `made` is the pane a request that makes one would have made, under the name it was sent
    /// with, so whoever asked can find it or close it rather than ask for a second.
    Unanswered { detail: String, made: Option<PaneId> },

    /// Anything else. The request did not happen, and saying so is all there is to do.
    Declined(String),
}

impl Refusal {
    /// What the backend said, for a log or a message back to whoever asked.
    pub fn detail(&self) -> &str {
        match self {
            Refusal::NotThere(detail)
            | Refusal::Unanswered { detail, .. }
            | Refusal::Declined(detail) => detail,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.detail())
    }
}

/// How big a pane's terminal is, in cells: the size its program last saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
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
    fn read(&self, pane: &PaneId, scope: Scope) -> Result<PaneText, Refusal>;

    /// How big every pane's terminal is, by pane. A read, for the reason [`Self::read`] is.
    ///
    /// Only the daemon can answer for a pane nobody is drawing: it holds each terminal at the
    /// size the last bridge to draw it asked for, or the size it was made at.
    fn grids(&self) -> Result<BTreeMap<PaneId, Grid>, Refusal>;

    /// What the prompt of a pane's agent holds, read as its daemon reads it before typing there:
    /// nothing for a prompt that is empty or cannot be read, and from a backend that cannot read
    /// one. A read, for the reason [`Self::read`] is.
    fn prompt(&self, _pane: &PaneId) -> Result<Option<String>, Refusal> {
        Ok(None)
    }

    /// Hears the callsigns every other machine this window follows holds, for the panes it asks
    /// for after to be drawn none of them. A backend with no callsigns has nothing to do.
    fn callsigns_elsewhere(&self, _held: Vec<String>) {}

    /// What this channel is talking to, for the log.
    fn description(&self) -> &str;
}
