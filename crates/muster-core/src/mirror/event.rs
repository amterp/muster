//! What a daemon tells Muster changed.
//!
//! The daemon's own events: whole records rather than deltas, numbered and delivered in order,
//! from a snapshot onwards with nothing replayed from before it (MIP-3, section 9). So an event
//! is applied as it stands, and a mirror never has to decide whether one is old news.

use crate::AgentState;
use crate::mirror::backend::{Pane, PaneId, Progress, Tab, TabId};

/// One thing a daemon says happened.
///
/// Every variant carries absolute values rather than deltas, so applying one twice is applying
/// it once.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// A pane exists. Its tab is whichever tree names it; the daemon names a pane in a tree only
    /// after it has opened it.
    PaneOpened(Pane),
    /// A pane's whole record, as it is now.
    PaneChanged(Pane),
    /// A pane is gone, however it went: closed by a request, or its program ended.
    PaneClosed(PaneId),
    TabOpened(Tab),
    /// A tab's whole record: its label, its tree, its zoom.
    TabChanged(Tab),
    TabClosed(TabId),
    /// The daemon has finished bringing back its saved tabs, and says what it could not.
    Restored(Restored),
    /// A paste the daemon held back, because the pane's program did not ask for bracketed
    /// paste and the text holds a newline, so writing it would run every line as typed. It
    /// is written once somebody confirms it.
    PasteHeld {
        pane: PaneId,
        text: String,
    },
    /// A program asked to set the clipboard (OSC 52).
    ClipboardWrite {
        pane: PaneId,
        text: String,
    },
    /// A program rang the bell.
    Bell {
        pane: PaneId,
    },
    /// A program asked for a desktop notification (OSC 9 or OSC 777).
    Notified {
        pane: PaneId,
        title: String,
        body: String,
    },
    /// A program said how far along it is (OSC 9;4), or that it is done saying: `None`.
    Progress {
        pane: PaneId,
        progress: Option<Progress>,
    },
}

/// What a daemon could not bring back from its saved state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored {
    pub lost_tabs: Vec<TabId>,
    /// Every pane that did not come back, a lost tab's among them.
    pub lost_panes: Vec<PaneId>,
    /// The daemon stopped saving, so what changes from now on is not written down.
    pub saving_stopped: bool,
}

/// What applying an event actually changed.
///
/// Returned so that rendering costs the change rather than a walk of every pane
/// (`architecture.md`: fast is a feature, the per-event half). An event that changed nothing
/// produces nothing here, which is what makes idempotence observable rather than merely
/// intended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    PaneAdded(PaneId),
    PaneRemoved(PaneId),
    AgentStateChanged {
        pane: PaneId,
        from: AgentState,
        to: AgentState,
    },
    /// The daemon set or cleared this pane's `finished_unseen`, which moves it between `done`
    /// and what its agent state says.
    FinishedUnseen {
        pane: PaneId,
        unseen: bool,
    },
    /// What this pane is called has moved - its name, its directory, or the harness detected
    /// in it.
    ///
    /// Not a state change: an agent that has just been recognized was already doing whatever
    /// it was doing, and a pane that changed directory is the same pane. It is reported
    /// because a list of panes names them by exactly these things, and a name that never
    /// updates is a pane the user cannot find twice.
    PaneRelabelled(PaneId),
    /// What is known of this pane's agent beyond its state has moved: what the agent says about
    /// itself, whether its state is its own report, or whether its screen can be read.
    ///
    /// Announced with the state, and for the same reason the state is kept out of the roster:
    /// an agent reports how full its context is as often as its statusline redraws.
    AgentDescribed(PaneId),
    TabAdded(TabId),
    /// What this tab is called has moved. The same shape as [`Change::PaneRelabelled`] and
    /// for the same reason: a caption that never updates is a tab somebody named and cannot
    /// find again.
    TabRelabelled(TabId),
    TabRemoved(TabId),
    /// This tab's tree, or which of its panes is zoomed, is not what it was. Carries the tab
    /// rather than the tree, because every reader has the mirror in hand and only some of
    /// them want to walk it.
    LayoutChanged(TabId),
    /// The daemon finished restoring. A daemon still restoring is not an empty one, so
    /// nothing asks it for a first tab until this arrives.
    Restored(Restored),
    /// A paste is waiting for somebody to confirm it. Passed through rather than held: the
    /// mirror is what the daemon holds, and this is a question for whoever is looking.
    PasteHeld {
        pane: PaneId,
        text: String,
    },
    /// A program asked to set the clipboard. Passed through for the same reason as
    /// `PasteHeld`: it is an effect, and the mirror holds none.
    ClipboardWrite {
        pane: PaneId,
        text: String,
    },
    /// A program in this pane rang the bell. Passed through: whether it marks the pane is the
    /// window's to say, since only the window knows whether somebody is looking at it. So it
    /// announces nothing by itself, and the window announces the pane when a bell marks it.
    Rang(PaneId),
    /// A program in this pane asked to notify somebody. Passed through, like `Rang`.
    Notified {
        pane: PaneId,
        title: String,
        body: String,
    },
    /// What a program in this pane says of its progress has moved. Announced with the pane's
    /// agent, as its facts are, since it blinks as often.
    ProgressChanged(PaneId),
}

impl Change {
    /// What kind of change this is, for a log line that has to say why the window moved.
    pub fn kind(&self) -> &'static str {
        match self {
            Change::PaneAdded(_) => "pane_added",
            Change::PaneRemoved(_) => "pane_removed",
            Change::AgentStateChanged { .. } => "agent_state",
            Change::FinishedUnseen { .. } => "finished_unseen",
            Change::PaneRelabelled(_) => "pane_relabelled",
            Change::AgentDescribed(_) => "agent_described",
            Change::TabAdded(_) => "tab_added",
            Change::TabRelabelled(_) => "tab_relabelled",
            Change::TabRemoved(_) => "tab_removed",
            Change::LayoutChanged(_) => "layout_changed",
            Change::Restored(_) => "restored",
            Change::PasteHeld { .. } => "paste_held",
            Change::ClipboardWrite { .. } => "clipboard_write",
            Change::Rang(_) => "rang",
            Change::Notified { .. } => "notified",
            Change::ProgressChanged(_) => "progress",
        }
    }

    /// Whether this can have moved something composition names.
    ///
    /// Agent state cannot: it is a property of a pane that still exists. Everything else that
    /// moves a tab or a pane can, and both are things a region is holding on to.
    ///
    /// A false positive costs a reconcile and a republish that change nothing. A false
    /// negative leaves a region pointing at a tab the daemon has closed, which is why the
    /// unfamiliar case belongs on the true side.
    pub fn moves_structure(&self) -> bool {
        !matches!(
            self,
            Change::AgentStateChanged { .. }
                | Change::FinishedUnseen { .. }
                | Change::AgentDescribed(_)
                | Change::PaneRelabelled(_)
                | Change::TabRelabelled(_)
                | Change::PasteHeld { .. }
                | Change::ClipboardWrite { .. }
                | Change::Rang(_)
                | Change::Notified { .. }
                | Change::ProgressChanged(_)
        )
    }

    /// Whether what the window is showing would come out different.
    ///
    /// A superset of [`Change::moves_structure`], and the two are separate because they
    /// answer different questions. Composition has to be reconciled when something it names
    /// may have moved; the view and the roster have to be republished whenever anything in
    /// them would read differently - and a pane's name is in the roster without being
    /// anywhere composition can see.
    ///
    /// Agent state is the one thing in neither. It has a message of its own for exactly this
    /// reason: republishing the whole arrangement every time an agent blinked is the
    /// per-event cost the budget is drawn against, and a full window of agents is the common
    /// case rather than the rare one.
    pub fn republishes(&self) -> bool {
        self.moves_structure()
            || matches!(self, Change::PaneRelabelled(_) | Change::TabRelabelled(_))
    }

    /// The pane whose agent state the shell has to be told about, if any.
    ///
    /// A transition is the obvious case. A pane appearing is the one that is easy to miss,
    /// and was: a pane already working when Muster attaches has never transitioned, so a
    /// shell told only about transitions paints a busy agent as `unknown` until that agent
    /// happens to move again.
    ///
    /// The state is not carried here because the mirror already holds it, and a second copy
    /// travelling beside the pane id is a second copy to disagree.
    pub fn announces_agent_state(&self) -> Option<&PaneId> {
        match self {
            Change::AgentStateChanged { pane, .. }
            | Change::FinishedUnseen { pane, .. }
            | Change::AgentDescribed(pane)
            | Change::ProgressChanged(pane)
            | Change::PaneAdded(pane) => Some(pane),
            _ => None,
        }
    }
}
