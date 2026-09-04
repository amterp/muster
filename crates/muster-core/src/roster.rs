//! Every pane every attached daemon holds, whether or not a window is showing it.
//!
//! The view answers "what is on screen". This answers "what exists", which is the other
//! half of the founding desideratum: states are glanceable only if the things carrying them
//! are all in one place, and a pane no region shows is exactly the one most likely to have
//! finished without anybody noticing.
//!
//! Structure only, like the view and for the same reason. What an agent is doing travels on
//! its own per-pane message, because a roster is mostly stable and a state blinks - joining
//! them would republish the whole list every time an agent moved. The shell holds both and
//! puts them together, which it already does to paint a pane's border.
//!
//! Ordered and labelled here rather than in the shell. Both are decisions: which pane a
//! person sees first, and what a pane is called when its id means nothing to anybody. A
//! shell that sorted for itself would be a second place those decisions live, and the CLI
//! and the agent-facing API would each need their own copy (`architecture.md`, one action
//! path).
//!
//! **Tab, then pane.** A flat list of panes cannot say which of them sit side by side in one
//! tab, which is the question "where has that agent got to" actually asks - and a window shows
//! one tab at a time, so the tab is the thing a person navigates between. The nesting is here
//! rather than rebuilt by each reader for the same reason the order is: it is a decision, and
//! the sidebar, the CLI and an agent must not each make their own.
//!
//! **The machine is not a level of it.** A tab may hold panes on two machines (MIP-2), so
//! grouping by machine would be a list that no longer describes the window beside it. Which
//! machine holds a pane is on the pane, and a machine holding no panes at all is in `machines`
//! below - a state that would otherwise have nowhere to be said.

use crate::composition::{Composition, DaemonId, PaneKey};
use crate::input::NumberedChords;
use crate::mirror::Mirror;
use crate::mirror::backend::Health;
use crate::mirror::backend::{Pane, PaneId, TabId};

/// Everything the attached daemons hold, in the order to show it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roster {
    /// The window's tabs, in the order it walks them.
    pub tabs: Vec<RosterTab>,

    /// The machines behind them, for the states no pane can carry.
    ///
    /// A machine that is connected and holding panes says nothing here that its panes do not
    /// already say; what needs somewhere to go is a machine that is unreachable, and a machine
    /// holding nothing at all. Without a per-machine heading over the tabs, the second would
    /// vanish from the window entirely, and a machine you asked to see and cannot use is the
    /// bug `a_2HpkpfIfq` was about.
    pub machines: Vec<RosterMachine>,
}

/// One attached machine, as something to say a word about and somewhere to put a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterMachine {
    pub id: DaemonId,

    /// `connected`, `stale` or `disconnected`, as the mirror answers it.
    pub health: Health,

    /// How many panes it holds, on screen or not. Zero is the state worth drawing.
    pub panes: usize,
}

/// One tab, as something to list and something to go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterTab {
    pub id: TabId,

    /// The machines holding panes in it, in the order their regions sit on screen.
    ///
    /// One for every tab until somebody groups two. What a reader wants it for is the tab
    /// itself saying nothing about machines: a tab may span them, so the machine goes on the
    /// pane row, and this is how anything that needs the set gets it without walking the panes.
    pub daemons: Vec<DaemonId>,

    /// Where this tab sits in the window's whole tab order, counting from one.
    ///
    /// The order `next_tab` walks, and what a tab nobody has named is called. Under the scheme
    /// Muster ships no chord names it: ⌘1 to ⌘9 number panes, because the rows carrying the
    /// agent states are pane rows and two numberings in one sidebar is worse than either.
    /// Whether a chord names it at this moment is [`Numbering::on_tab`], not this. Counted
    /// across every daemon rather than within one, because a window showing a laptop beside a
    /// devenv is one list.
    pub place: usize,

    /// What to call this tab to somebody who did not open it.
    pub label: String,

    /// Whether this is the tab the window is showing.
    ///
    /// Exactly one tab carries it, because a window shows one at a time. Not the same question
    /// as any of its panes being on screen: a zoomed tab is on screen while all but one of its
    /// panes are not, and that is the honest reading of both - the tab is what the window shows,
    /// and a pane is what the tree inside it renders.
    pub on_screen: bool,

    /// The name somebody gave this tab, if anybody has, on the same terms as a pane's.
    ///
    /// Not derivable from `label` above, which may carry the tab's workspace in front of the
    /// name and drops a name that is only digits. Renaming has to start from what was typed.
    pub given_name: Option<String>,

    pub panes: Vec<RosterPane>,
}

/// One pane, as something to list rather than something to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPane {
    pub key: PaneKey,

    /// Where this pane sits in the window's whole pane order, counting from one.
    ///
    /// The handle ⌘1 to ⌘9 name under the scheme Muster ships, so the fourth row down the
    /// sidebar and ⌘4 are one pane. Counted across every daemon and every tab rather than
    /// within one, because the sidebar is one list and somebody reading it counts down the
    /// whole thing.
    ///
    /// Positional: it is read off the row rather than remembered, and it moves when a pane
    /// opens or closes above it. That is the cost of numbering the thing that churns, and it
    /// is the right trade once the order is yours to arrange - a number that stayed put when
    /// you dragged its row somewhere else would be fighting you.
    ///
    /// Numbered past nine even though no chord goes that far, because the number is what the
    /// pane's position *is*; what to draw is the sidebar's decision and it stops at nine.
    pub place: usize,

    /// Where this pane sits among its own tab's panes, counting from one.
    ///
    /// The same position [`RosterPane::place`] is, read within one tab instead of down the
    /// whole window. Both are here because they answer different questions and a reader
    /// should not have to count rows to get the second: the sidebar reads down the list and
    /// wants the first, and anything scoped to a tab wants this.
    pub place_in_tab: usize,

    /// What to call this pane to somebody who did not open it.
    pub label: String,
    /// What its agent is working on, when that is worth a line of its own.
    ///
    /// Absent for most panes, and that is the design rather than a gap: a second line on
    /// every row doubles the height of a list whose whole value is being glanceable. See
    /// [`pane_subtitle`] for when one is drawn.
    pub subtitle: Option<String>,
    /// The name somebody gave this pane, if anybody has - the raw text, not the composed
    /// label above.
    ///
    /// Carried so that asking to rename a pane can start from what it is already called
    /// rather than from `muster · claude`, which nobody typed. Absent means unnamed, which
    /// is what "has a name" is asked of without a sentinel.
    pub given_name: Option<String>,
    /// Whether a region is showing it right now.
    ///
    /// Here rather than left to the shell to work out by comparing against the view: the two
    /// messages arrive separately and a shell joining them would render a pane as hidden for
    /// as long as they disagreed. It is also the thing the list is for - a pane nobody is
    /// showing is the one worth going to.
    pub on_screen: bool,
}

/// What the numbered chords name right now.
///
/// One value with three states rather than a scheme plus a flag, because the rule this holds
/// up is that only one thing may be numbered at a time. Split into two values, a reader would
/// have to combine them to answer "what does ⌘2 do", and the sidebar and the chord could
/// combine them differently - which is exactly the disagreement the settled scheme was
/// designed to make impossible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Numbering {
    /// Every pane, counted down the whole window. What Muster does.
    #[default]
    Panes,

    /// Every tab, counted across the window. The prototype scheme, waiting for a first press.
    Tabs,

    /// The panes inside one tab. The prototype scheme, after a first press named that tab.
    PanesIn(TabId),
}

impl Numbering {
    /// What the chords name, given the scheme, the tab a press named, and what exists.
    ///
    /// Derived from all three every time rather than remembered, so the order things happen in
    /// stops mattering: a tab that closed while a press had named it is simply not in the
    /// roster, and the answer falls back to numbering tabs. Deciding this once, at the moment
    /// an input arrived, is the shape of a bug this codebase has shipped before.
    ///
    /// **A window holding one tab numbers panes under either scheme.** ⌘1 would otherwise be
    /// spent naming the only tab there is, and reaching that tab's second pane would take ⌘1 ⌘2,
    /// a first press carrying no information every time, for as long as the window holds one tab
    /// (kan a_2Hx68fXqr). With one tab a pane's place down the whole window and its place inside
    /// that tab are the same number, so this is not a third behaviour: it is the settled scheme,
    /// which under these conditions the prototype agrees with.
    ///
    /// Said as `Panes` rather than as `PanesIn(the only tab)` for what follows from it. A
    /// numbering that says panes-inside-a-tab also says a chord is half-typed: the shell draws a
    /// number over every pane while one is, and ends the gesture when the modifier comes up. No
    /// chord has been typed here at all, so `PanesIn` would leave those numbers drawn over every
    /// pane forever.
    ///
    /// One tab in the *window*, not one tab on the machine the keyboard is on. Per machine the
    /// same key would mean different things depending on which column the keyboard sat in, with
    /// nothing on screen changing as it moved between them. Per window the meaning changes only
    /// when a second tab appears, which at least moves every number in the sidebar as it
    /// happens - and this function is handed the roster and knows nothing about the keyboard,
    /// which is the shape the other reading would have to break.
    pub fn of(scheme: NumberedChords, named: Option<&TabId>, roster: &Roster) -> Numbering {
        match scheme {
            NumberedChords::Panes => Numbering::Panes,
            NumberedChords::TabThenPane if roster.tabs().count() == 1 => Numbering::Panes,
            NumberedChords::TabThenPane => match named {
                Some(key) if roster.tabs().any(|tab| &tab.id == key) => {
                    Numbering::PanesIn(key.clone())
                }
                _ => Numbering::Tabs,
            },
        }
    }

    /// Which ⌘N reaches this tab right now, if any does.
    pub fn on_tab(&self, tab: &RosterTab) -> Option<usize> {
        match self {
            Numbering::Tabs => Some(tab.place),
            Numbering::Panes | Numbering::PanesIn(_) => None,
        }
    }

    /// Which ⌘N reaches this pane right now, if any does.
    ///
    /// Takes the tab holding it because two of the three answers are about the tab rather
    /// than the pane, and a pane does not carry its own tab.
    pub fn on_pane(&self, tab: &RosterTab, pane: &RosterPane) -> Option<usize> {
        match self {
            Numbering::Panes => Some(pane.place),
            Numbering::Tabs => None,
            Numbering::PanesIn(key) => (&tab.id == key).then_some(pane.place_in_tab),
        }
    }
}

/// Where a numbered chord lands, and what the press after it will name.
///
/// The second half is why this is not just a pane: reaching a tab has to leave the numbering
/// somewhere different from where it found it, and only whatever resolved the chord knows
/// that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing<'a> {
    /// This pane, and the numbering stays where it was.
    Pane(&'a RosterPane),

    /// This tab, landing on the pane named, and its panes are what gets numbered next.
    Tab(&'a RosterTab, &'a RosterPane),
}

impl Landing<'_> {
    /// The pane the keyboard goes to, which every landing has one of.
    pub fn pane(&self) -> &RosterPane {
        match self {
            Landing::Pane(pane) | Landing::Tab(_, pane) => pane,
        }
    }

    /// The tab this press named, for the press after it to count inside.
    ///
    /// `None` after landing on a pane, so the next press starts over at whatever the scheme
    /// numbers first. That is what keeps the sequence two deep: three ⌘2s in a row are the
    /// second tab, its second pane, and the second tab again, rather than descending into
    /// something with no third level to descend into.
    ///
    /// `None` too for a tab holding one pane, because there is nothing in it to choose
    /// between: the press has already landed on the only pane there is. Naming it would leave
    /// the window in a state whose whole content is one number nobody needs, and would spend
    /// the press after it on a chord that can only miss.
    pub fn named(&self) -> Option<TabId> {
        match self {
            Landing::Pane(_) => None,
            Landing::Tab(tab, _) if tab.panes.len() < 2 => None,
            Landing::Tab(tab, _) => Some(tab.id.clone()),
        }
    }
}

/// Which way a step through the window's tabs goes.
///
/// Two directions and no more, unlike [`crate::composition::Step`]. Tabs are a list and not an
/// arrangement - nothing is to the left of a tab - so the four geometric directions have
/// nothing to mean here. Both wrap, because the list has no edge worth bumping against and
/// between them they have to reach every tab: that reachability is the whole reason these
/// exist, since a pane in a tab no region shows can otherwise be reached only by mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStep {
    Next,
    Previous,
}

impl TabStep {
    /// The name a chord, a menu item and a CLI all spell it with.
    pub fn parse(name: &str) -> Option<TabStep> {
        match name {
            "next" => Some(TabStep::Next),
            "previous" => Some(TabStep::Previous),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TabStep::Next => "next",
            TabStep::Previous => "previous",
        }
    }

    /// Every step there is, so a test can assert nothing has been left unspelled.
    pub const ALL: [TabStep; 2] = [TabStep::Next, TabStep::Previous];
}

impl Roster {
    /// Builds the list from every followed daemon's mirror.
    ///
    /// A closure rather than a map of mirrors, for the same reason [`crate::composition::View`]
    /// takes one: in a running app each mirror is behind its own lock, and only the caller
    /// knows how to hold them.
    ///
    /// Daemons come in the order their regions sit in the window, so the list reads down the
    /// side in the order the panes read across it, and a daemon with no region on screen
    /// follows. Sorting by id instead would be stable and arbitrary - the window is right
    /// there, and a list that disagreed with it would be one more thing to reconcile by eye.
    pub fn of<'a>(
        composition: &Composition,
        mirror: impl Fn(&DaemonId) -> Option<&'a Mirror>,
        showing: &std::collections::BTreeSet<PaneKey>,
    ) -> Roster {
        let on_screen = composition.showing().cloned();

        // Two counters, both running across every tab: panes are what the chords name, and
        // tabs are numbered only so that one nobody has named still has something to be called.
        let mut tab_place = 0;
        let mut pane_place = 0;
        let tabs = composition
            .tabs()
            .map(|tab| {
                tab_place += 1;
                let panes: Vec<RosterPane> = tab
                    .regions()
                    .filter_map(|region| Some((&region.daemon, mirror(&region.daemon)?)))
                    .flat_map(|(daemon, held)| {
                        ordered_panes(held, &tab.id)
                            .into_iter()
                            .map(move |pane| (daemon, pane))
                            .collect::<Vec<_>>()
                    })
                    .enumerate()
                    .map(|(at, (daemon, pane))| {
                        let key = PaneKey::new(daemon, &pane.id);
                        let label = pane_label(pane);
                        pane_place += 1;
                        RosterPane {
                            place: pane_place,
                            place_in_tab: at + 1,
                            subtitle: pane_subtitle(pane, &label),
                            label,
                            given_name: given_name(pane.name.as_deref()),
                            on_screen: showing.contains(&key),
                            key,
                        }
                    })
                    .collect();
                // The first machine holding panes in it answers for the tab's caption. A tab
                // that spans two is one tab with one name, and its members are renamed together
                // - so which of them is read is a question only about which has answered.
                let first = tab.daemons().next().and_then(|daemon| Some((daemon, mirror(daemon)?)));
                let own = first.and_then(|(_, held)| held.tab(&tab.id));
                RosterTab {
                    id: tab.id.clone(),
                    daemons: tab.daemons().cloned().collect(),
                    place: tab_place,
                    label: match (first, own) {
                        (Some((_, held)), Some(own)) => {
                            tab_label(held, own, names_its_workspaces(held), tab_place)
                        }
                        _ => format!("Tab {tab_place}"),
                    },
                    on_screen: on_screen.as_ref() == Some(&tab.id),
                    given_name: own.and_then(tab_own_name).and_then(|name| given_name(Some(name))),
                    panes,
                }
            })
            .collect();

        let machines = composition
            .daemons()
            .map(|daemon| {
                let held = mirror(&daemon.id);
                RosterMachine {
                    id: daemon.id.clone(),
                    health: held.map_or(Health::Disconnected, Mirror::health),
                    panes: held.map_or(0, |held| held.panes().count()),
                }
            })
            .collect();

        Roster { tabs, machines }
    }

    /// Every tab in the window, in the order they are numbered.
    pub fn tabs(&self) -> impl Iterator<Item = &RosterTab> {
        self.tabs.iter()
    }

    /// Every pane in the window, in the order they are listed.
    pub fn panes(&self) -> impl Iterator<Item = &RosterPane> {
        self.tabs().flat_map(|tab| tab.panes.iter())
    }

    /// The pane at a given place in the order, counting from one.
    ///
    /// `None` for a place past the end, which is what a numbered chord in a window with fewer
    /// panes means. Doing nothing is the right answer there: landing on the last pane instead
    /// would make ⌘9 mean something different every time a pane opened.
    pub fn at(&self, place: usize) -> Option<&RosterPane> {
        self.panes().find(|pane| pane.place == place)
    }

    /// Where the numbered chord for `place` lands, under the numbering in force.
    ///
    /// Answered by looking for the row carrying that number rather than by indexing into
    /// whatever is being counted. Slower by a walk of fifteen rows, and worth it: it is the
    /// same pair of functions the sidebar draws its numbers from, so the number you can see
    /// beside a row is by construction the number that reaches it. A second implementation
    /// here would be a second place that rule lives, and the two would eventually disagree.
    ///
    /// `None` for a place past the end of whatever is being counted, which is what ⌘9 means
    /// in a window of two. Doing nothing is the answer in every branch, for the reason
    /// [`Roster::at`] gives: a chord that landed elsewhere once the list grew would mean
    /// something different every time a pane opened. An armed tab that has since closed lands
    /// here too, which is what un-arms it.
    pub fn numbered(&self, numbering: &Numbering, place: usize) -> Option<Landing<'_>> {
        for tab in self.tabs() {
            if numbering.on_tab(tab) == Some(place) {
                // A tab with no panes has nothing for the keyboard to land on, so the chord
                // does nothing rather than arming a tab you cannot then get into.
                return tab.panes.first().map(|pane| Landing::Tab(tab, pane));
            }
            if let Some(pane) =
                tab.panes.iter().find(|pane| numbering.on_pane(tab, pane) == Some(place))
            {
                return Some(Landing::Pane(pane));
            }
        }
        None
    }

    /// Where the keyboard goes when stepping one tab from the one it is on.
    ///
    /// Wraps, unlike the four geometric directions and like next and previous pane: this is
    /// the guarantee that every tab is reachable, and a step that silently did nothing at the
    /// end of the list is indistinguishable from a dead key.
    ///
    /// Stepping from a tab that is not in the list - nothing focused, or a tab that closed
    /// while the keystroke was in flight - goes to the end it came from rather than refusing,
    /// on the same terms as [`crate::composition::View::step`].
    pub fn step(&self, from: Option<&TabId>, direction: TabStep) -> Option<&RosterTab> {
        let order: Vec<&RosterTab> = self.tabs().collect();
        let at = from.and_then(|key| order.iter().position(|tab| &tab.id == key));
        match at {
            Some(at) => {
                let step = match direction {
                    TabStep::Next => 1,
                    TabStep::Previous => order.len().checked_sub(1)?,
                };
                order.get((at + step) % order.len()).copied()
            }
            None => match direction {
                TabStep::Next => order.first().copied(),
                TabStep::Previous => order.last().copied(),
            },
        }
    }
}

/// Whether this daemon's workspaces have names worth putting in front of a tab.
///
/// herdr labels a workspace with its directory, which is the useful half of a tab's name, and
/// labels a tab with its number within that workspace. So a daemon holding one workspace would
/// repeat that directory down the whole list, and a daemon holding several needs it on every
/// row to tell `muster · 1` from `rad · 1`.
fn names_its_workspaces(mirror: &Mirror) -> bool {
    mirror.workspaces().filter(|workspace| !workspace.label.is_empty()).count() > 1
}

/// The part of a tab's backend label that is somebody's name for it rather than its number.
///
/// herdr gives every tab a label whether or not anyone named it, filling in the tab's
/// position within its workspace, so "is this named" cannot be asked of presence. The
/// all-digits test is what separates the two, and it is why a tab somebody names `42`
/// reads as unnamed: herdr's `TabInfo` offers no way to tell those apart
/// (`observations/herdr-0.8.0.md` section 16).
fn tab_own_name(tab: &crate::mirror::backend::Tab) -> Option<&str> {
    let own = tab.label.trim();
    (!own.is_empty() && !own.chars().all(|c| c.is_ascii_digit())).then_some(own)
}

/// Trims a backend's answer and reads blank as absent, so "has a name" is one question.
fn given_name(name: Option<&str>) -> Option<String> {
    name.map(str::trim).filter(|name| !name.is_empty()).map(str::to_string)
}

/// What to call a tab to somebody who did not open it.
///
/// The workspace first, because it is the project the tab belongs to and the only part of a
/// tab's name that means anything on sight - and only when the daemon holds more than one,
/// since repeating one project's name down every row says nothing and costs a word off every
/// label.
///
/// **A tab nobody has named is called `Tab <place>`, which Muster writes rather than reads.**
/// herdr labels an unnamed tab with its position inside its workspace, so the label of the
/// second tab is literally `2`, and [`tab_own_name`] drops it - a bare digit is not a name,
/// and two daemons would each contribute a `1`. Muster's own place counts across the whole
/// window, so the caption is unique and agrees with the order the list is read in.
///
/// This used to be empty, back when a number was drawn beside every caption and the row was
/// never blank. The numbers now name panes, so an empty answer here would be a row with
/// nothing on it - which is a worse failure than a wrong one, because there is nothing to
/// notice.
fn tab_label(
    mirror: &Mirror,
    tab: &crate::mirror::backend::Tab,
    named: bool,
    place: usize,
) -> String {
    let own = tab_own_name(tab).unwrap_or_default();
    let workspace = named
        .then(|| mirror.workspaces().find(|held| held.id == tab.workspace))
        .flatten()
        .map(|workspace| workspace.label.trim())
        .filter(|label| !label.is_empty())
        .unwrap_or_default();
    match (workspace, own) {
        // Nothing to call it, so Muster writes one. The place is not a chord any more - the
        // numbers name panes - so this is a name rather than a second thing to press.
        ("", "") => format!("Tab {place}"),
        ("", own) => own.to_string(),
        (workspace, "") => workspace.to_string(),
        (workspace, own) => format!("{workspace} · {own}"),
    }
}

/// One tab's panes, in the order they are laid out.
///
/// The tree decides, so the list reads the way the splits do. A tab whose tree has not
/// arrived, or whose tree disagrees with the panes it holds, falls back to the pane list in
/// its own order - the panes exist and belong on the list either way, and an arrangement
/// nobody has described yet is not a reason to hide them (`architecture.md`, a tree that
/// disagrees with its tab is not an arrangement).
fn ordered_panes<'a>(mirror: &'a Mirror, tab: &'a TabId) -> Vec<&'a Pane> {
    let held: Vec<&Pane> = mirror.panes_in_tab(tab).collect();
    let Some(layout) = mirror.layout(tab) else { return held };
    let arranged: Vec<&PaneId> = layout.root.panes();
    if arranged.len() != held.len() {
        return held;
    }
    let mut ordered = Vec::with_capacity(held.len());
    for id in arranged {
        match held.iter().find(|pane| &pane.id == id) {
            Some(pane) => ordered.push(*pane),
            None => return held,
        }
    }
    ordered
}

/// What to call a pane to somebody who did not open it.
///
/// **A name somebody gave it wins**, because it is the only line here that was written by a
/// person for this pane rather than derived from where it happens to be. It is also the
/// durable one: herdr writes a name down, so it comes back after a daemon restart, where
/// everything below is worked out afresh each time (`observations/herdr-0.8.0.md` section
/// 16).
///
/// Failing that, the directory first, because for a window full of coding agents that is
/// what tells two panes apart - the ids are `w1:p1` and `w1:p2`, which say nothing. The
/// harness follows when one was detected, because "which of these is the one running claude"
/// is the other question asked of a list like this.
///
/// The id is the last resort rather than the first, and it is better than an empty row: a
/// pane with no directory is still a pane somebody has to be able to point at.
///
/// Public because a notification names a pane too, and it has to be the same name. Building
/// a whole roster to read one row would lock every attached daemon for a banner.
pub fn pane_label(pane: &Pane) -> String {
    if let Some(name) = pane.name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    format!("{}{}", pane_directory(pane), harness_suffix(pane))
}

fn pane_directory(pane: &Pane) -> &str {
    let directory = pane.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    if directory.is_empty() { pane.id.as_str() } else { directory }
}

fn harness_suffix(pane: &Pane) -> String {
    match &pane.agent {
        Some(agent) if !agent.is_empty() => format!(" · {agent}"),
        _ => String::new(),
    }
}

/// The second line of a pane's row: what its agent is working on, when that is worth a line.
///
/// The founding promise is that fifteen panes can be told apart at a glance, and fifteen rows
/// reading `<directory> · claude` cannot do it. A harness already publishes what it is doing
/// as its terminal title, so the material is there - the decision is when drawing it earns
/// the height, and it is made here rather than in a sidebar so that the CLI and an agent get
/// the same answer as the window (`architecture.md`, attention routing).
///
/// **Only a pane with a detected harness gets one.** A plain shell sets a title too, and
/// oh-my-zsh's default is `<user>@<host>:<path>` - the row's own first line, spelled longer.
/// Suppressing that by matching on shell prompt conventions would be a guess about somebody's
/// dotfiles; requiring a harness is a fact the daemon reports. What it costs is stated rather
/// than hidden: a pane running something that titles itself usefully, which herdr recognized
/// no agent in, stays on one line.
///
/// **And only when it says something the first line does not.** A harness that titles itself
/// after the directory - which Claude does - would otherwise draw the same word twice at
/// double the height.
/// `label` is what the row's first line ended up saying, passed in rather than composed again:
/// this runs once per pane on every roster, which is once per title change across the window.
///
/// Public on the same terms as [`pane_label`], and for the same caller.
pub fn pane_subtitle(pane: &Pane, label: &str) -> Option<String> {
    let agent = pane.agent.as_deref().filter(|agent| !agent.is_empty())?;
    let title = pane.title.as_deref().map(str::trim).filter(|title| !title.is_empty())?;

    let repeats = |said: &str| said.eq_ignore_ascii_case(title);
    let already_said = repeats(label)
        || repeats(pane_directory(pane))
        || repeats(pane.cwd.trim_end_matches('/'))
        || repeats(agent);
    (!already_said).then(|| title.to_string())
}
