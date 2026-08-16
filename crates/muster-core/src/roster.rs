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
//! **Daemon, then tab, then pane.** A flat list of panes cannot say which of them sit side by
//! side in one tab, which is the question "where has that agent got to" actually asks - and a
//! window shows one tab per region, so the tab is the thing a person navigates between. The
//! nesting is here rather than rebuilt by each reader for the same reason the order is: it is
//! a decision, and the sidebar, the CLI and an agent must not each make their own.

use crate::composition::{Composition, DaemonId, PaneKey, TabKey};
use crate::mirror::Mirror;
use crate::mirror::backend::{Pane, PaneId, TabId};

/// Everything the attached daemons hold, in the order to show it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roster {
    pub daemons: Vec<RosterDaemon>,
}

/// One attached daemon, and the tabs it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterDaemon {
    pub id: DaemonId,
    pub tabs: Vec<RosterTab>,
}

/// One tab, as something to list and something to go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterTab {
    pub key: TabKey,

    /// Where this tab sits in the window's whole tab order, counting from one.
    ///
    /// The handle a numbered chord names, and the same order `next_tab` walks - so the third
    /// row down the sidebar, ⌘3, and two presses of `next_tab` from the first all mean one
    /// tab. Counted across every daemon rather than within one, because a window showing a
    /// laptop beside a devenv has one list and a person reading it counts down the whole
    /// thing.
    ///
    /// It moves when a tab opens or closes ahead of it, which every numbered-tab scheme has
    /// to live with: a number is a position, and positions shift.
    pub place: usize,

    /// What to call this tab to somebody who did not open it.
    pub label: String,

    /// Whether a region is showing this tab right now.
    ///
    /// Not the same question as any of its panes being on screen. A zoomed tab is on screen
    /// while all but one of its panes are not, and that is the honest reading of both: the tab
    /// is what a region shows, and a pane is what the tree inside it renders.
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
        let mut ordered: Vec<&DaemonId> = Vec::new();
        for region in composition.regions() {
            if !ordered.contains(&&region.daemon) {
                ordered.push(&region.daemon);
            }
        }
        for daemon in composition.daemons() {
            if !ordered.contains(&&daemon.id) {
                ordered.push(&daemon.id);
            }
        }

        let on_screen: std::collections::BTreeSet<TabKey> =
            composition.regions().map(|region| TabKey::new(&region.daemon, &region.tab)).collect();

        let mut place = 0;
        let daemons = ordered
            .into_iter()
            .filter_map(|daemon| Some((daemon, mirror(daemon)?)))
            .map(|(daemon, held)| {
                let named = names_its_workspaces(held);
                let tabs = held
                    .tabs()
                    .map(|tab| {
                        place += 1;
                        RosterTab {
                            key: TabKey::new(daemon, &tab.id),
                            place,
                            label: tab_label(held, tab, named),
                            on_screen: on_screen.contains(&TabKey::new(daemon, &tab.id)),
                            given_name: given_name(tab_own_name(tab)),
                            panes: ordered_panes(held, &tab.id)
                                .into_iter()
                                .map(|pane| {
                                    let key = PaneKey::new(daemon, &pane.id);
                                    RosterPane {
                                        label: pane_label(pane),
                                        subtitle: pane_subtitle(pane),
                                        given_name: given_name(pane.name.as_deref()),
                                        on_screen: showing.contains(&key),
                                        key,
                                    }
                                })
                                .collect(),
                        }
                    })
                    .collect();
                RosterDaemon { id: daemon.clone(), tabs }
            })
            .collect();
        Roster { daemons }
    }

    /// Every tab in the window, in the order they are numbered.
    ///
    /// The flat reading of the tree, which is what moving between tabs is about: the nesting
    /// is for a reader, and a keystroke asking for the next one does not care which machine
    /// answers.
    pub fn tabs(&self) -> impl Iterator<Item = &RosterTab> {
        self.daemons.iter().flat_map(|daemon| daemon.tabs.iter())
    }

    /// Every pane in the window, in the order they are listed.
    pub fn panes(&self) -> impl Iterator<Item = &RosterPane> {
        self.tabs().flat_map(|tab| tab.panes.iter())
    }

    /// The tab at a given place in the order, counting from one.
    ///
    /// `None` for a place past the end, which is what a numbered chord in a window with fewer
    /// tabs means. Doing nothing is the right answer there: jumping to the last tab instead
    /// would make ⌘9 mean something different every time a tab opened.
    pub fn at(&self, place: usize) -> Option<&RosterTab> {
        self.tabs().find(|tab| tab.place == place)
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
    pub fn step(&self, from: Option<&TabKey>, direction: TabStep) -> Option<&RosterTab> {
        let order: Vec<&RosterTab> = self.tabs().collect();
        let at = from.and_then(|key| order.iter().position(|tab| &tab.key == key));
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

/// What to call a tab to somebody who did not open it.
///
/// The workspace first, because it is the project the tab belongs to and the only part of a
/// tab's name that means anything on sight - and only when the daemon holds more than one,
/// since repeating one project's name down every row says nothing and costs a word off every
/// label.
///
/// **A tab herdr has not named is left nameless rather than given a number.** herdr labels an
/// unnamed tab with its position inside its workspace, so the label of the second tab is
/// literally `2` - and every row carries a place of its own already, drawn beside it. Passing
/// that through produced captions reading `2 · 2`, which was measured in the running app
/// rather than reasoned about. Muster's place is the better number: it counts across the whole
/// window, which is what the numbered chords use. A name that is only digits is therefore
/// dropped, and the row is its number.
///
/// The empty answer is a real one and not a hole: nothing here is ever the only thing a row
/// has, because the place is always drawn. A tab id would fit the space and tell nobody
/// anything.
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

fn tab_label(mirror: &Mirror, tab: &crate::mirror::backend::Tab, named: bool) -> String {
    let own = tab_own_name(tab).unwrap_or_default();
    let workspace = named
        .then(|| mirror.workspaces().find(|held| held.id == tab.workspace))
        .flatten()
        .map(|workspace| workspace.label.trim())
        .filter(|label| !label.is_empty())
        .unwrap_or_default();
    match (workspace, own) {
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
fn pane_label(pane: &Pane) -> String {
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
fn pane_subtitle(pane: &Pane) -> Option<String> {
    let agent = pane.agent.as_deref().filter(|agent| !agent.is_empty())?;
    let title = pane.title.as_deref().map(str::trim).filter(|title| !title.is_empty())?;

    let repeats = |said: &str| said.eq_ignore_ascii_case(title);
    let already_said = repeats(&pane_label(pane))
        || repeats(pane_directory(pane))
        || repeats(pane.cwd.trim_end_matches('/'))
        || repeats(agent);
    (!already_said).then(|| title.to_string())
}
