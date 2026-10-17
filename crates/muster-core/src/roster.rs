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

use std::collections::BTreeMap;

use crate::composition::{Composition, DaemonId, PaneKey};
use crate::config::Rgb;
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

    /// The tabs no window shows, which are what hiding a pane makes: each pane hidden on its
    /// own tab, and any pane made beside one in it.
    ///
    /// Apart from `tabs` because nothing about them is the window's: no chord reaches them, no
    /// place numbers them, and nothing draws them, so every `place` here is 0 and every
    /// `on_screen` false. Listed so that what their agents are doing is still in view and each
    /// can be brought back. Every window lists the same ones, because hiding is a daemon's fact.
    pub hidden: Vec<RosterTab>,
}

/// One attached machine, as something to say a word about and somewhere to put a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterMachine {
    pub id: DaemonId,

    /// `connected`, `stale` or `disconnected`, as the mirror answers it.
    pub health: Health,

    /// How many panes it holds, on screen or not. Zero is the state worth drawing.
    pub panes: usize,

    /// What its last restart cost, in a few words, while that restart still needs somebody
    /// ([`crate::mirror::Restart::outstanding`]). The other state worth a row of its own.
    pub restarted: Option<String>,
}

/// The color that marks a machine's rows in the agent list.
///
/// What the config file chose for it when it chose one, and otherwise a color drawn from its
/// name: the same machine gets the same color in every window and every launch, because the
/// only input is the name. Drawn from a name rather than handed out in config order, so a
/// machine keeps its color when another is added or removed (amterp, kan a_2Mi2uZQUx).
///
/// Two names can land on one color, and there are six to go round. That is what the config
/// file's `color` is for.
pub fn machine_color(id: &DaemonId, chosen: &BTreeMap<DaemonId, Rgb>) -> Rgb {
    if let Some(chosen) = chosen.get(id) {
        return *chosen;
    }
    let hash = fnv1a(id.to_string().as_bytes());
    MACHINE_COLORS[usize::try_from(hash).unwrap_or_default() % MACHINE_COLORS.len()]
}

/// What a machine's mark may be painted in when the file says nothing.
///
/// Chosen to stay clear of the colors Muster ships for agent states - cyan, orange, indigo,
/// green and grey - because the mark sits on the same row as a state's dot, and a machine
/// that looked like `working` would be a row that lies. Six rather than more: past six, hues
/// stop being told apart at the size of a swatch.
const MACHINE_COLORS: [Rgb; 6] = [
    Rgb { red: 0xff, green: 0x4f, blue: 0x9a },
    Rgb { red: 0xf2, green: 0xc2, blue: 0x00 },
    Rgb { red: 0xa8, green: 0x78, blue: 0x4f },
    Rgb { red: 0xb2, green: 0x5c, blue: 0xe6 },
    Rgb { red: 0xe5, green: 0x48, blue: 0x4d },
    Rgb { red: 0x8d, green: 0x94, blue: 0x40 },
];

/// FNV-1a over 32 bits. Written out rather than taken from `std`, because `std`'s hasher is
/// free to change between Rust releases and a machine's color must not.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash, &byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193))
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
    /// The order `next_tab` walks, what a tab nobody has named is called, and the press ⌘1 to ⌘9
    /// name it by once the window holds more than one tab. Whether a chord names it at this
    /// moment is [`Numbering::on_tab`], not this. Counted
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
    /// Not derivable from `label` above, which is `Tab <place>` when nobody has named the tab,
    /// so a tab somebody named `Tab 3` reads the same as one nobody named. Renaming has to
    /// start from what was typed.
    pub given_name: Option<String>,

    pub panes: Vec<RosterPane>,
}

impl RosterTab {
    /// Whether reaching this tab leaves a press outstanding.
    ///
    /// A tab holding one pane does not: the press has already landed on the only pane there is,
    /// so naming it would spend the press after it on a chord that can only miss. Here rather
    /// than spelled inline where it is needed, because two readers now depend on it and they
    /// have to agree - [`Landing::named`] decides what a press leaves behind, and
    /// [`Numbering::chord_on_pane`] decides what the sidebar draws beside the pane. A tab that
    /// armed by one rule and drew by the other would show a chord that does not reach it.
    pub fn arms(&self) -> bool {
        self.panes.len() > 1
    }
}

/// One pane, as something to list rather than something to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPane {
    pub key: PaneKey,

    /// Where this pane sits in the window's whole pane order, counting from one.
    ///
    /// What `muster window` prints beside the pane and `muster focus --place` takes back, and
    /// what ⌘1 to ⌘9 name in a window of one tab. Counted across every daemon and every tab rather than
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
    /// What to type to reach this pane: the name somebody gave it, or the word its daemon drew.
    /// Absent only for a pane nobody named on a daemon from before words.
    pub name: Option<String>,
    /// The word its daemon drew (`otter`), whether or not somebody has named it since. Absent
    /// only on a daemon from before words.
    ///
    /// Carried apart from `label` so that a row can draw it in a style of its own and keep it in
    /// view through a rename: the daemon keeps a renamed pane's word reserved for it, so it is
    /// still one pane's on its machine (`docs/cli/overview.md`).
    pub word: Option<String>,
    /// What a row drawing `word` on its own puts after it: [`pane_beside_word`]. The same as
    /// `label` for a pane with no word.
    pub beside_word: String,
    /// Whether a region is showing it right now.
    ///
    /// Here rather than left to the shell to work out by comparing against the view: the two
    /// messages arrive separately and a shell joining them would render a pane as hidden for
    /// as long as they disagreed. It is also the thing the list is for - a pane nobody is
    /// showing is the one worth going to.
    pub on_screen: bool,
    /// Whether its agent can be compacted, as its daemon says: what a menu offering to compact it
    /// greys itself out on.
    pub compactable: bool,
    /// What its daemon would run to resume its agent's session, words joined by spaces, when it
    /// knows: what a menu offering to restart the agent greys itself out on, and what the window
    /// says before it restarts one.
    pub resumes_as: Option<String>,
    /// Whether it raises nothing for the person, as its daemon says.
    pub quiet: bool,
    /// The pane that made it as its helper, as its daemon says (MIP-7).
    pub maker: Option<PaneId>,
    /// The row it is listed beneath rather than where its tab puts it: its maker's, for a hidden
    /// helper whose maker has a row of its own ([`crate::helpers::nest`]).
    pub under: Option<PaneId>,
}

/// What the next press names.
///
/// One value with three states rather than a scheme plus a flag, because the rule this holds
/// up is that a press means exactly one thing at a time. Split into two values, a reader would
/// have to combine them to answer "what does ⌘2 do", and two readers could combine them
/// differently.
///
/// **What a press names and what the sidebar draws are two questions now.** They used to be
/// one: the numbers sat wherever the next press could reach, so they moved between the tab
/// rows and a tab's pane rows as a chord was typed. A pane in a tab nobody was looking at
/// therefore had no readable address at all, which is what kan a_2LSUoy7dd took out. So
/// [`Numbering::on_tab`] and [`Numbering::on_pane`] answer the press, and
/// [`Numbering::chord_on_tab`] and [`Numbering::chord_on_pane`] answer the row - the second
/// pair built out of the first, so the two cannot drift.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Numbering {
    /// Every pane, counted down the whole window. A window holding one tab, where a pane's
    /// place in the window and its place in the tab are the same number.
    #[default]
    Panes,

    /// Every tab, counted across the window, waiting for a first press.
    Tabs,

    /// The panes inside one tab, after a first press named that tab.
    PanesIn(TabId),
}

impl Numbering {
    /// What the chords name, given the tab a press named and what exists.
    ///
    /// Derived from all three every time rather than remembered, so the order things happen in
    /// stops mattering: a tab that closed while a press had named it is simply not in the
    /// roster, and the answer falls back to numbering tabs. Deciding this once, at the moment
    /// an input arrived, is the shape of a bug this codebase has shipped before.
    ///
    /// **A window holding one tab numbers its panes.** ⌘1 would otherwise be
    /// spent naming the only tab there is, and reaching that tab's second pane would take ⌘1 ⌘2,
    /// a first press carrying no information every time, for as long as the window holds one tab
    /// (kan a_2Hx68fXqr). With one tab a pane's place down the whole window and its place inside
    /// that tab are the same number, so this is the same chord drawn with one press less.
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
    pub fn of(named: Option<&TabId>, roster: &Roster) -> Numbering {
        if roster.tabs().count() == 1 {
            return Numbering::Panes;
        }
        match named {
            Some(key) if roster.tabs().any(|tab| &tab.id == key) => Numbering::PanesIn(key.clone()),
            _ => Numbering::Tabs,
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

    /// The whole chord that reaches this tab, which is one press or none.
    ///
    /// This is the tab's place whether or not a press is outstanding,
    /// which is where it parts company with [`Numbering::on_tab`] above. ⌘4 stops meaning tab 4
    /// for as long as a press is outstanding, and saying so by taking the digit away would
    /// mean the sidebar's numbers move as a chord is typed - which is the thing kan
    /// a_2LSUoy7dd took out. What says it instead is [`Numbering::armed_on`], which the shell
    /// draws as emphasis rather than as position.
    pub fn chord_on_tab(&self, tab: &RosterTab) -> Chord {
        match self {
            Numbering::Panes => Chord::none(),
            Numbering::Tabs | Numbering::PanesIn(_) => {
                Chord::naming_tab(Numbering::Tabs.on_tab(tab))
            }
        }
    }

    /// The whole chord that reaches this pane, in the order the presses are made.
    ///
    /// Composed from the same two functions [`Roster::numbered`] resolves a press through, one
    /// numbering at a time, so a chord drawn beside a row is by construction the presses that
    /// reach it. A second implementation of "what gets you there" is the disagreement the
    /// numbers exist to prevent.
    ///
    /// Nothing here consults what has already been pressed. That is the point: a pane in a tab
    /// nobody is looking at has a readable address, and the address does not change under
    /// somebody reading it.
    pub fn chord_on_pane(&self, tab: &RosterTab, pane: &RosterPane) -> Chord {
        match self {
            Numbering::Panes => Chord::naming_pane(Numbering::Panes.on_pane(tab, pane)),
            Numbering::Tabs | Numbering::PanesIn(_) => {
                let Chord { tab: Some(first), .. } = self.chord_on_tab(tab) else {
                    // No press reaches the tab, so no sequence reaches anything inside it. Said
                    // as nothing at all rather than as a pane press alone: a lone second digit
                    // is a keystroke that would land somewhere else entirely.
                    return Chord::none();
                };
                if !tab.arms() {
                    // The press that reaches the tab lands on its only pane and stops, so it is
                    // the whole chord. Drawing a second digit here would name another tab.
                    return Chord { tab: Some(first), pane: None };
                }
                let second = Numbering::PanesIn(tab.id.clone()).on_pane(tab, pane);
                Chord::within(first, second)
            }
        }
    }

    /// Whether the next press names a pane inside this tab.
    ///
    /// What a window says instead of moving its numbers. True for exactly one tab, and only
    /// while a press is outstanding.
    pub fn armed_on(&self, tab: &RosterTab) -> bool {
        matches!(self, Numbering::PanesIn(key) if key == &tab.id)
    }
}

/// The presses that reach one row, in the order a hand makes them.
///
/// Two named presses rather than a list, because two is as deep as a chord goes and a
/// `Vec` would make every reader handle lengths neither can produce.
///
/// **A press outside ⌘1 to ⌘9 is no press at all**, and takes the rest of the chord with it.
/// Those places exist as positions - a window holds a tenth pane, and the roster numbers it -
/// but no chord reaches them, and half a chord drawn beside a row is worse than none: it is a
/// keystroke that looks like it goes there and goes somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Chord {
    /// The press that names the tab. None in a window of one tab,
    /// where one press names a pane and no press names a tab.
    pub tab: Option<usize>,

    /// The press that names the pane: inside its tab, or down the whole window when the window
    /// holds one tab.
    pub pane: Option<usize>,
}

impl Chord {
    /// Nothing reaches this row.
    fn none() -> Chord {
        Chord::default()
    }

    /// One press, naming a tab.
    fn naming_tab(tab: Option<usize>) -> Chord {
        Chord { tab: tab.filter(|&place| Chord::reachable(place)), pane: None }
    }

    /// One press, naming a pane.
    fn naming_pane(pane: Option<usize>) -> Chord {
        Chord { tab: None, pane: pane.filter(|&place| Chord::reachable(place)) }
    }

    /// A tab's press and a pane's inside it, where an unreachable second leaves neither.
    fn within(tab: usize, pane: Option<usize>) -> Chord {
        match pane.filter(|&place| Chord::reachable(place)) {
            Some(pane) => Chord { tab: Some(tab), pane: Some(pane) },
            None => Chord::none(),
        }
    }

    /// Whether a place is one of the nine a chord can name.
    fn reachable(place: usize) -> bool {
        (1..=9).contains(&place)
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
    /// `None` after landing on a pane, so the next press starts over at whatever the window
    /// numbers first. That is what keeps the sequence two deep: three ⌘2s in a row are the
    /// second tab, its second pane, and the second tab again, rather than descending into
    /// something with no third level to descend into.
    ///
    /// `None` too for a tab that does not arm, which is [`RosterTab::arms`] and its reasons.
    pub fn named(&self) -> Option<TabId> {
        match self {
            Landing::Pane(_) => None,
            Landing::Tab(tab, _) if !tab.arms() => None,
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
        let tabs: Vec<RosterTab> = composition
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
                            name: pane_name(pane).map(str::to_string),
                            word: pane_word(pane).map(str::to_string),
                            beside_word: pane_beside_word(pane),
                            on_screen: showing.contains(&key),
                            compactable: pane.compactable,
                            resumes_as: resumes_as(pane),
                            quiet: pane.quiet,
                            maker: pane.maker.clone(),
                            under: None,
                            key,
                        }
                    })
                    .collect();
                // A tab spanning two machines holds its name on each, and the part with the
                // highest generation has the current one; a lagging part is renamed when its
                // daemon reconnects (MIP-3, section 2).
                let own = tab
                    .daemons()
                    .filter_map(|daemon| mirror(daemon)?.tab(&tab.id))
                    .max_by_key(|part| part.generation);
                let name = own.and_then(tab_name);
                RosterTab {
                    id: tab.id.clone(),
                    daemons: tab.daemons().cloned().collect(),
                    place: tab_place,
                    label: name.map_or_else(|| format!("Tab {tab_place}"), str::to_string),
                    on_screen: on_screen.as_ref() == Some(&tab.id),
                    given_name: name.map(str::to_string),
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
                    restarted: held
                        .filter(|held| held.restart_outstanding())
                        .and_then(Mirror::last_restart)
                        .map(crate::mirror::Restart::summary),
                }
            })
            .collect();

        let mut hidden = hidden_tabs(composition, &mirror);
        nest_helpers(&tabs, &mut hidden);

        Roster { tabs, machines, hidden }
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
    /// What `muster focus --place` goes to. `None` for a place past the end: landing on the last
    /// pane instead would send a stale place somewhere different every time a pane opened.
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

/// Every hidden tab the window's daemons hold, a tab spanning machines once, in the order the
/// daemons are attached and then the order each opened its tabs.
///
/// A hidden tab nobody named has an empty label rather than a number, since nothing places it,
/// and is listed under a heading that already says what it is.
fn hidden_tabs<'a>(
    composition: &Composition,
    mirror: &impl Fn(&DaemonId) -> Option<&'a Mirror>,
) -> Vec<RosterTab> {
    let mut hidden: Vec<RosterTab> = Vec::new();
    for daemon in composition.daemons() {
        let Some(held) = mirror(&daemon.id) else { continue };
        for part in held.tabs().filter(|tab| tab.hidden) {
            let at = if let Some(at) = hidden.iter().position(|tab| tab.id == part.id) {
                at
            } else {
                let name = tab_name(part);
                hidden.push(RosterTab {
                    id: part.id.clone(),
                    daemons: Vec::new(),
                    place: 0,
                    label: name.unwrap_or_default().to_string(),
                    on_screen: false,
                    given_name: name.map(str::to_string),
                    panes: Vec::new(),
                });
                hidden.len() - 1
            };
            let tab = &mut hidden[at];
            tab.daemons.push(daemon.id.clone());
            for pane in ordered_panes(held, &part.id) {
                let label = pane_label(pane);
                tab.panes.push(RosterPane {
                    key: PaneKey::new(&daemon.id, &pane.id),
                    place: 0,
                    place_in_tab: tab.panes.len() + 1,
                    subtitle: pane_subtitle(pane, &label),
                    label,
                    given_name: given_name(pane.name.as_deref()),
                    name: pane_name(pane).map(str::to_string),
                    word: pane_word(pane).map(str::to_string),
                    beside_word: pane_beside_word(pane),
                    on_screen: false,
                    compactable: pane.compactable,
                    resumes_as: resumes_as(pane),
                    quiet: pane.quiet,
                    maker: pane.maker.clone(),
                    under: None,
                });
            }
        }
    }
    hidden
}

/// Says which hidden helpers the list shows under their maker, on each helper's `under`.
///
/// Only hidden ones: a helper in one of the window's tabs keeps its place there, and the number a
/// chord reaches it by, which a row moved under its maker would put out of order.
fn nest_helpers(tabs: &[RosterTab], hidden: &mut [RosterTab]) {
    let under: Vec<(PaneId, PaneId)> = {
        let listed: Vec<&str> = tabs
            .iter()
            .chain(hidden.iter())
            .flat_map(|tab| tab.panes.iter())
            .map(|pane| pane.key.pane.as_str())
            .collect();
        let makers: Vec<(&str, Option<&str>)> = hidden
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .map(|pane| (pane.key.pane.as_str(), pane.maker.as_ref().map(PaneId::as_str)))
            .collect();
        crate::helpers::nest(&listed, &makers)
            .into_iter()
            .map(|nested| (PaneId::from(nested.helper), PaneId::from(nested.under)))
            .collect()
    };
    for pane in hidden.iter_mut().flat_map(|tab| tab.panes.iter_mut()) {
        pane.under =
            under.iter().find(|(helper, _)| *helper == pane.key.pane).map(|(_, m)| m.clone());
    }
}

/// What somebody named a tab, trimmed, and nothing for a tab nobody named.
///
/// **A tab nobody has named is captioned `Tab <place>`, which Muster writes rather than reads.**
/// Muster's own place counts across the whole window, so the caption is unique and agrees with
/// the order the list is read in, where two machines would each have a first tab.
fn tab_name(tab: &crate::mirror::backend::Tab) -> Option<&str> {
    tab.label.as_deref().map(str::trim).filter(|name| !name.is_empty())
}

/// Trims a backend's answer and reads blank as absent, so "has a name" is one question.
fn given_name(name: Option<&str>) -> Option<String> {
    name.map(str::trim).filter(|name| !name.is_empty()).map(str::to_string)
}

/// The command a pane's agent would be restarted with, for a person to read rather than to run:
/// its words joined by spaces, unquoted.
fn resumes_as(pane: &Pane) -> Option<String> {
    pane.resume.as_ref().map(|resume| resume.command.join(" "))
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
    let Some(layout) = mirror.tab(tab) else { return held };
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
/// person for this pane rather than derived from where it happens to be. It also survives a
/// daemon restart, which the harness below does not: the daemon writes a name down (MIP-3,
/// section 2).
///
/// Failing that, the directory first, because for a window full of coding agents that is
/// what tells two panes apart - the names are `p1w3r07bsd` and `p1w3r0ab2n`, which say
/// nothing. The harness follows when one was detected, because "which of these is the one
/// running claude" is the other question asked of a list like this.
///
/// The id is the last resort rather than the first, and it is better than an empty row: a
/// pane with no directory is still a pane somebody has to be able to point at.
///
/// Public because a notification names a pane too, and it has to be the same name. Building
/// a whole roster to read one row would lock every attached daemon for a banner.
///
/// **A pane going by the word its daemon drew leads with the word**, because it is what you
/// type to reach the pane, and keeps the directory and the harness after it, because a word
/// says nothing about the work and those are what tell fifteen rows apart.
pub fn pane_label(pane: &Pane) -> String {
    let beside = pane_beside_word(pane);
    match pane_word(pane).filter(|_| given_name(pane.name.as_deref()).is_none()) {
        Some(word) if beside.is_empty() => word.to_string(),
        Some(word) => format!("{word} · {beside}"),
        None => beside,
    }
}

/// What a row that draws a pane's word on its own puts after the word: the name somebody gave the
/// pane, or else its directory and harness - the label with the word taken out, so the word is
/// drawn once. A pane with no word gets its whole label.
///
/// The label is built from this, so the two cannot come to disagree about anything but the word.
pub fn pane_beside_word(pane: &Pane) -> String {
    if let Some(name) = given_name(pane.name.as_deref()) {
        return name;
    }
    let harness = pane.agent.as_deref().filter(|agent| !agent.is_empty());
    // With no directory, a word is enough to point at the pane by, where the id it would fall
    // back to says nothing a reader can use.
    if pane_word(pane).is_some() && pane.cwd.trim_end_matches('/').is_empty() {
        return harness.unwrap_or_default().to_string();
    }
    format!("{}{}", pane_directory(pane), harness_suffix(pane))
}

/// The word a pane's daemon drew, if it has one.
fn pane_word(pane: &Pane) -> Option<&str> {
    pane.word.as_deref().filter(|word| !word.is_empty())
}

/// What a pane is called, as something to type: the name somebody gave it, or failing that the
/// word its daemon drew. The daemon's own reading of a record (`muster_daemon_proto::Pane::name`)
/// says the same, which it has to: a name is an address.
pub fn pane_name(pane: &Pane) -> Option<&str> {
    pane.name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| pane.word.as_deref().filter(|word| !word.is_empty()))
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
/// than hidden: a pane running something that titles itself usefully, in which the daemon
/// recognized no agent, stays on one line.
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
