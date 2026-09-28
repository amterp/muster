//! Who is there, for the message service: an agent in a pane as this daemon's detection reads
//! the pane, and anyone else by whether their inbox still answers (MIP-4, section 7).
//!
//! The panes are read once per request, with the session held and then let go, before the
//! message service's own lock is taken: the two locks are never held together.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use muster_daemon_proto as proto;
use muster_msg::{Activity, Participant, Presence};

use super::inbox;
use crate::pane::PaneIo;
use crate::session::Shared;

/// A pane with an agent in it, as it was when it was read.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// Its agent's state, or nothing when detection has not settled on one.
    pub(crate) activity: Option<Activity>,
    /// Its agent, as detection names it.
    pub(crate) agent: String,
    pub(crate) io: Arc<PaneIo>,
}

impl Seen {
    /// When anything was last typed or sent into the pane.
    pub(crate) fn input_at(&self) -> Option<Instant> {
        self.io.input_at()
    }
}

/// Every pane with an agent in it, by name, and the names of the rest.
#[derive(Debug, Default)]
pub(crate) struct Panes {
    agents: HashMap<String, Seen>,
    open: HashSet<String>,
}

impl Panes {
    pub(crate) fn of(shared: &Shared) -> Panes {
        let read = shared.lock().each_pane(|record, io| {
            let seen = record.agent.clone().filter(|_| !io.is_closed()).map(|agent| Seen {
                activity: activity(record),
                agent,
                io: io.clone(),
            });
            (record.pane.clone(), seen)
        });
        let mut panes = Panes::default();
        for (pane, seen) in read {
            if let Some(seen) = seen {
                panes.agents.insert(pane.clone(), seen);
            }
            panes.open.insert(pane);
        }
        panes
    }

    pub(crate) fn get(&self, pane: &str) -> Option<&Seen> {
        self.agents.get(pane)
    }

    /// Whether the pane is open, agent or not: one whose agent has not been found yet, after a
    /// restart say, is worth waiting for.
    pub(crate) fn exists(&self, pane: &str) -> bool {
        self.open.contains(pane)
    }
}

fn activity(record: &proto::Pane) -> Option<Activity> {
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
        self.open.contains(pane)
    }
}
