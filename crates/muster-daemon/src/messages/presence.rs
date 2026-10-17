//! Who is there, for the message service: an agent in a pane as this daemon's detection reads
//! the pane, and anyone else by whether their inbox still answers (MIP-4, section 7).
//!
//! The panes are read once per request, with the session held and then let go, before the
//! message service's own lock is taken: the two locks are never held together.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_daemon_proto as proto;
use muster_detect::Agent;
use muster_msg::{Activity, Participant, Presence, Ringable};

use super::inbox;
use crate::compaction::Step;
use crate::pane::PaneIo;
use crate::session::Shared;

/// A pane with an agent in it, as it was when it was read.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// Its agent's state, or nothing when detection has not settled on one.
    pub(crate) activity: Option<Activity>,
    /// Its agent, as detection names it.
    pub(crate) agent: String,
    /// Whether its agent's manifest can read its prompt, without which it is never rung.
    pub(crate) rings: bool,
    /// The pane's name, still to be given to its agent's session, and the line its manifest
    /// says to type for it.
    pub(crate) rename: Option<Rename>,
    /// The line its manifest says compacts its agent, or the prompt owed once that has run, still
    /// to be typed.
    pub(crate) compact: Option<Step>,
    /// The id its agent reported for its session, when its manifest names a command that wakes
    /// the session by it (`super::command`).
    pub(crate) session: Option<String>,
    /// The pane's program, when it has stopped reading its input, and what it left unread.
    pub(crate) stopped: Option<(String, u32)>,
    pub(crate) io: Arc<PaneIo>,
}

/// A pane's name to be typed into its agent's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rename {
    pub(crate) name: String,
    pub(crate) line: String,
}

impl Seen {
    /// When anything was last typed or sent into the pane.
    pub(crate) fn input_at(&self) -> Option<Instant> {
        self.io.input_at()
    }

    /// When the pane's program last drew anything.
    pub(crate) fn drawn_at(&self) -> Option<Instant> {
        self.io.drawn_at()
    }
}

/// How long after a pane is made an agent is still expected in it: `muster pane new --run
/// claude` and then a post to it come before detection has found the agent (MIP-4, section 14).
/// A pane that has had no agent for longer has none coming.
const AGENT_TO_COME: Duration = Duration::from_secs(30);

/// Every pane with an agent in it, by name, and the names of the rest, each with whether an
/// agent may still be coming to it.
#[derive(Debug, Default)]
pub(crate) struct Panes {
    agents: HashMap<String, Seen>,
    open: HashMap<String, bool>,
    /// What each open pane is called, by its name.
    names: HashMap<String, String>,
    /// What each open pane with a title shows in its title bar, by its name.
    titles: HashMap<String, String>,
    /// Whether a window is attending, which wakes the human.
    attended: bool,
    /// Whether a restore is still bringing panes back: a pane not open yet may be one of them,
    /// with its agent resuming in it.
    restoring: bool,
    /// The pane each pane a zoom hides is behind, by the hidden pane.
    zoomed: HashMap<String, String>,
    /// The pane each helper was made by, by the helper (MIP-7), while that has not closed: the
    /// maker may be on another machine.
    makers: HashMap<String, String>,
}

impl Panes {
    pub(crate) fn of(shared: &Shared) -> Panes {
        // Until the manifests load, nothing can say whether an agent's prompt is readable, so no
        // agent is found yet and one may still come to any pane.
        let manifests = shared.detecting.manifests();
        let loaded = manifests.is_some();
        let session = shared.lock();
        let naming = session.names_sessions();
        let read = session.each_pane(|pane| {
            let (record, io) = (&pane.record, &pane.io);
            let agent = record.agent.clone().filter(|_| loaded && !io.is_closed());
            let seen = agent.map(|agent| {
                let found = Agent::new(&agent);
                let rename = pane.session_name.wanted().filter(|_| naming).and_then(|name| {
                    let line = manifests.as_ref()?.session_rename(&found, name)?;
                    Some(Rename { name: name.to_string(), line })
                });
                let session = pane.session_id.clone().filter(|id| {
                    manifests
                        .as_ref()
                        .is_some_and(|manifests| manifests.session_wake(&found, id, "").is_some())
                });
                Seen {
                    activity: activity(record),
                    rings: manifests
                        .as_ref()
                        .is_some_and(|manifests| manifests.reads_prompt(&found)),
                    rename,
                    compact: pane.compaction.wanted(),
                    session,
                    stopped: record
                        .unread_input
                        .as_ref()
                        .map(|unread| (unread.program.clone(), unread.bytes)),
                    agent,
                    io: io.clone(),
                }
            });
            let name = record.name().map(str::to_string);
            let title = Some(record.title.clone()).filter(|title| !title.trim().is_empty());
            let young = !loaded || io.age() < AGENT_TO_COME;
            let maker = record.maker.clone().filter(|_| !record.maker_closed);
            (record.pane.clone(), seen, young, name, title, maker)
        });
        let mut panes = Panes {
            attended: session.attended(),
            restoring: session.restoring(),
            zoomed: session.zoomed_away().into_iter().collect(),
            ..Panes::default()
        };
        drop(session);
        for (pane, seen, young, name, title, maker) in read {
            if let Some(maker) = maker {
                panes.makers.insert(pane.clone(), maker);
            }
            if let Some(seen) = seen {
                panes.agents.insert(pane.clone(), seen);
            }
            if let Some(name) = name {
                panes.names.insert(pane.clone(), name);
            }
            if let Some(title) = title {
                panes.titles.insert(pane.clone(), title);
            }
            panes.open.insert(pane, young);
        }
        panes
    }

    pub(crate) fn get(&self, pane: &str) -> Option<&Seen> {
        self.agents.get(pane)
    }

    /// Every pane with an agent in it.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Seen)> {
        self.agents.iter()
    }

    /// Whether the pane is open, agent or not.
    pub(crate) fn exists(&self, pane: &str) -> bool {
        self.open.contains_key(pane)
    }

    /// Every helper open here whose maker has not closed, with the pane that made it.
    pub(crate) fn makers(&self) -> impl Iterator<Item = (&String, &String)> {
        self.makers.iter()
    }

    /// The pane `pane`'s tab is zoomed on, when the zoom hides `pane`.
    pub(crate) fn zoomed_on(&self, pane: &str) -> Option<&str> {
        self.zoomed.get(pane).map(String::as_str)
    }
}

fn activity(record: &proto::Pane) -> Option<Activity> {
    // Whatever detection read: the screen shows what it showed before the program stopped.
    if record.unread_input.is_some() {
        return Some(Activity::Stuck);
    }
    let waiting = record.facts.as_ref().is_some_and(|facts| facts.waiting.is_some());
    match record.agent_state() {
        proto::AgentState::Working => Some(Activity::Working),
        proto::AgentState::Blocked => Some(Activity::Blocked),
        proto::AgentState::Idle if waiting => Some(Activity::Waiting),
        proto::AgentState::Idle => Some(Activity::Idle),
        proto::AgentState::Unknown => None,
    }
}

impl Presence for Panes {
    fn attended(&self) -> bool {
        self.attended
    }

    /// An agent in a pane is there while its pane has an agent in it, whatever its inbox says:
    /// the session in it exits with it. Anyone else is there while its inbox answers.
    fn alive(&self, participant: &Participant) -> bool {
        match &participant.pane {
            Some(pane) => self.agents.contains_key(pane),
            None => inbox::Sockets.alive(participant),
        }
    }

    fn activity(&self, participant: &Participant) -> Option<Activity> {
        participant.pane.as_ref().and_then(|pane| self.agents.get(pane)?.activity)
    }

    fn agent_in(&self, pane: &str) -> bool {
        self.agents.contains_key(pane)
    }

    fn has_pane(&self, pane: &str) -> bool {
        self.open.contains_key(pane)
    }

    fn name_of(&self, pane: &str) -> Option<String> {
        self.names.get(pane).cloned()
    }

    /// Only one pane, since the daemon gives no two live panes one name; a state file from before
    /// it did can bring two back, and then the name means neither.
    fn pane_named(&self, name: &str) -> Option<String> {
        let mut called = self.names.iter().filter(|(_, called)| *called == name);
        match (called.next(), called.next()) {
            (Some((pane, _)), None) => Some(pane.clone()),
            _ => None,
        }
    }

    fn titles(&self) -> Vec<(String, String)> {
        self.titles.iter().map(|(pane, title)| (pane.clone(), title.clone())).collect()
    }

    fn doorbell(&self, pane: &str) -> Ringable {
        match (self.agents.get(pane), self.open.get(pane)) {
            (Some(seen), _) if seen.rings || seen.session.is_some() => Ringable::Rings,
            (Some(_), _) => Ringable::NoPrompt,
            (None, Some(true)) => Ringable::AgentToCome,
            (None, None) if self.restoring => Ringable::AgentToCome,
            (None, _) => Ringable::NoAgent,
        }
    }
}
