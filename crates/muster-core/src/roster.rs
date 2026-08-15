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

use crate::composition::{Composition, DaemonId, PaneKey};
use crate::mirror::Mirror;
use crate::mirror::backend::{Pane, PaneId, TabId};

/// Everything the attached daemons hold, in the order to show it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roster {
    pub panes: Vec<RosterPane>,
}

/// One pane, as something to list rather than something to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPane {
    pub key: PaneKey,
    pub tab: TabId,
    /// What to call this pane to somebody who did not open it.
    pub label: String,
    /// Whether a region is showing it right now.
    ///
    /// Here rather than left to the shell to work out by comparing against the view: the two
    /// messages arrive separately and a shell joining them would render a pane as hidden for
    /// as long as they disagreed. It is also the thing the list is for - a pane nobody is
    /// showing is the one worth going to.
    pub on_screen: bool,
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
        let mut daemons: Vec<&DaemonId> = Vec::new();
        for region in composition.regions() {
            if !daemons.contains(&&region.daemon) {
                daemons.push(&region.daemon);
            }
        }
        for daemon in composition.daemons() {
            if !daemons.contains(&&daemon.id) {
                daemons.push(&daemon.id);
            }
        }

        let panes = daemons
            .into_iter()
            .filter_map(|daemon| Some((daemon, mirror(daemon)?)))
            .flat_map(|(daemon, held)| {
                held.tabs().flat_map(move |tab| {
                    ordered(held, &tab.id).into_iter().map(move |pane| {
                        let key = PaneKey::new(daemon, &pane.id);
                        RosterPane {
                            label: label(pane),
                            on_screen: showing.contains(&key),
                            key,
                            tab: tab.id.clone(),
                        }
                    })
                })
            })
            .collect();
        Roster { panes }
    }
}

/// One tab's panes, in the order they are laid out.
///
/// The tree decides, so the list reads the way the splits do. A tab whose tree has not
/// arrived, or whose tree disagrees with the panes it holds, falls back to the pane list in
/// its own order - the panes exist and belong on the list either way, and an arrangement
/// nobody has described yet is not a reason to hide them (`architecture.md`, a tree that
/// disagrees with its tab is not an arrangement).
fn ordered<'a>(mirror: &'a Mirror, tab: &'a TabId) -> Vec<&'a Pane> {
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
/// The directory first, because for a window full of coding agents that is what tells two
/// panes apart - the ids are `w1:p1` and `w1:p2`, which say nothing, and a terminal title
/// is whatever the program last felt like setting. The harness follows when one was
/// detected, because "which of these is the one running claude" is the other question asked
/// of a list like this.
///
/// The id is the last resort rather than the first, and it is better than an empty row: a
/// pane with no directory is still a pane somebody has to be able to point at.
fn label(pane: &Pane) -> String {
    let directory = pane.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    let directory = if directory.is_empty() { pane.id.as_str() } else { directory };
    match &pane.agent {
        Some(agent) if !agent.is_empty() => format!("{directory} · {agent}"),
        _ => directory.to_string(),
    }
}
