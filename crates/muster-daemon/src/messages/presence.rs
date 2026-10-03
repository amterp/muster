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
    /// Whether a window is attending, which wakes the human.
    attended: bool,
}

impl Panes {
    pub(crate) fn of(shared: &Shared) -> Panes {
        // Until the manifests load, nothing can say whether an agent's prompt is readable, so no
        // agent is found yet and one may still come to any pane.
        let manifests = shared.detecting.manifests();
        let loaded = manifests.is_some();
        let read = shared.lock().each_pane(|pane| {
            let (record, io) = (&pane.record, &pane.io);
            let agent = record.agent.clone().filter(|_| loaded && !io.is_closed());
            let seen = agent.map(|agent| {
                let found = Agent::new(&agent);
                let rename = pane.session_name.wanted().and_then(|name| {
                    let line = manifests.as_ref()?.session_rename(&found, name)?;
                    Some(Rename { name: name.to_string(), line })
                });
                Seen {
                    activity: activity(record),
                    rings: manifests.as_ref().is_some_and(|manifests| manifests.reads_prompt(&found)),
                    rename,
                    agent,
                    io: io.clone(),
                }
            });
            (record.pane.clone(), seen, !loaded || io.age() < AGENT_TO_COME)
        });
        let mut panes = Panes { attended: shared.lock().attended(), ..Panes::default() };
        for (pane, seen, young) in read {
            if let Some(seen) = seen {
                panes.agents.insert(pane.clone(), seen);
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

    fn doorbell(&self, pane: &str) -> Ringable {
        match (self.agents.get(pane), self.open.get(pane)) {
            (Some(seen), _) if seen.rings => Ringable::Rings,
            (Some(_), _) => Ringable::NoPrompt,
            (None, Some(true)) => Ringable::AgentToCome,
            (None, _) => Ringable::NoAgent,
        }
    }
}
