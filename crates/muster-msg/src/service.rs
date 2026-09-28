use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::names::{check_group, check_participant};
use crate::{
    Entry, GroupRecord, HUMAN, LARGEST_BODY, Policy, Refusal, Saved, Store, What, default_name,
    pair_group,
};

/// A Claude Code session's inbox socket. The socket's path is the session's process id, which
/// the system hands out again once that process is gone, so the file's inode goes with it: a
/// new session bound to a reused path is a different socket file, and is not taken for the old
/// participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub socket: String,
    pub inode: u64,
}

/// Who is asking, as far as its environment says (MIP-4, section 3).
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// `--as`, which outranks everything else.
    pub as_name: Option<String>,
    pub inbox: Option<Inbox>,
    /// The pane the caller runs in, by its name. It identifies the caller only while its host
    /// has found an agent there ([`Presence::agent_in`]).
    pub pane: Option<String>,
    /// The caller's working directory, which a default name is made from.
    pub directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<Inbox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// A wake failed to reach it. It keeps its name and cursors until it joins again.
    #[serde(default)]
    pub gone: bool,
    /// Per group, the sequence number of the last entry it has read.
    #[serde(default)]
    pub cursors: BTreeMap<String, u64>,
    /// Groups it has been woken for since it last read them: one wake per batch.
    #[serde(default)]
    pub woken: BTreeSet<String>,
    /// Of those, the groups it has been woken for a second time, having gone idle without
    /// reading: never a third (MIP-4, section 5).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub rewoken: BTreeSet<String>,
}

impl Participant {
    fn named(name: &str) -> Participant {
        Participant {
            name: name.to_string(),
            inbox: None,
            pane: None,
            gone: false,
            cursors: BTreeMap::new(),
            woken: BTreeSet::new(),
            rewoken: BTreeSet::new(),
        }
    }
}

/// What an agent in a pane is doing, as its host's detection reads the pane (MIP-4, section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Working,
    /// At a dialog: a keystroke would answer it.
    Blocked,
    Idle,
    /// Idle, having said it is waiting on work of its own.
    Waiting,
}

/// What the host can tell about participants: whether one is still there - for a Claude
/// session, whether its inbox still accepts a connection; for an agent in a pane, whether the
/// pane still has an agent in it - and, for a pane, what its agent is doing.
pub trait Presence {
    fn alive(&self, participant: &Participant) -> bool;

    /// What the agent at the participant's pane is doing, when it has one.
    fn activity(&self, _participant: &Participant) -> Option<Activity> {
        None
    }

    /// Whether `pane` names a pane with an agent in it, which identifies whoever runs a
    /// command there (MIP-4, section 3).
    fn agent_in(&self, _pane: &str) -> bool {
        false
    }

    /// Whether `pane` names an open pane, which may be addressed by that name before it has run
    /// any command, and before its agent has even been found: a pane just made is addressed at
    /// once (MIP-4, section 14). Its wakes wait until an agent there can take them.
    fn has_pane(&self, pane: &str) -> bool {
        self.agent_in(pane)
    }

    /// Whether the doorbell can reach whoever is in `pane` (MIP-4, section 6).
    fn doorbell(&self, pane: &str) -> Ringable {
        if self.agent_in(pane) {
            Ringable::Rings
        } else if self.has_pane(pane) {
            Ringable::AgentToCome
        } else {
            Ringable::NoAgent
        }
    }
}

/// Whether the doorbell can reach whoever is in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ringable {
    /// An agent is there, and the host can read its prompt.
    Rings,
    /// An agent is there whose prompt the host cannot read, so it is never rung.
    NoPrompt,
    /// No agent has been found there yet, in a pane new enough that one is likely starting.
    AgentToCome,
    /// No agent is there, and none is expected: the pane is closed, or its agent has left.
    NoAgent,
}

impl Ringable {
    /// Whether a wake for the pane is worth keeping to ring.
    fn rings(self) -> bool {
        matches!(self, Ringable::Rings | Ringable::AgentToCome)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Gone,
    /// The human, who counts as alive whether or not anyone is at the window, since messages
    /// to the human wait for them.
    Human,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub liveness: Liveness,
    pub activity: Option<Activity>,
    pub groups: Vec<String>,
    pub inbox: Option<String>,
    pub pane: Option<String>,
}

/// What a woken participant is told: never a body, since only its own `read` moves its cursor
/// (MIP-4, section 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub group: String,
    pub first: u64,
    pub last: u64,
    pub count: u64,
    pub to_you: u64,
    pub from: Vec<String>,
    /// Woken for these once already, and gone idle without reading them.
    pub again: bool,
}

/// How a wake reaches its participant (MIP-4, section 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// A line on a Claude Code session's inbox socket.
    Inbox(Inbox),
    /// A line typed into the pane the agent runs in, once its host's guards allow.
    Pane(String),
}

/// A wake for the host to deliver, outside whatever lock it holds this under, and to report
/// back with [`Messaging::delivered`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    pub name: String,
    pub via: Via,
    pub notice: Notice,
}

/// What a post did for each participant it was meant to wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Woken,
    /// To be woken once its host's guards allow: an agent in a pane that is busy, at a dialog,
    /// or being typed into. The host decides this, not the service.
    Deferred,
    /// Woken for this group already, and has not read since: it will read this too.
    AlreadyWoken,
    /// Nothing can wake it, so it sees the message when it next reads.
    Waiting,
    Gone,
    /// In a pane where no agent is, or is coming, to be rung, and with nothing else to wake it.
    NoAgent,
    /// In a pane whose agent's prompt the host cannot read, so it is never rung, and with
    /// nothing else to wake it.
    NoDoorbell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub author: String,
    pub group: String,
    pub seq: u64,
    pub reached: Vec<(String, Reach)>,
    pub wakes: Vec<Wake>,
    /// Waits this post ended, with what to tell each.
    pub answered: Vec<AnsweredWait>,
    /// Why the state beside the log could not be saved, when it could not. The message is in
    /// the log regardless, so the post happened; what is at risk is only who was woken, which
    /// costs at most a wake too many after a restart.
    pub unsaved: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredWait {
    pub ticket: u64,
    pub name: String,
    pub notice: Notice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub name: String,
    pub group: Option<String>,
    pub created: bool,
    pub took_over: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub name: String,
    pub groups: Vec<String>,
    /// Left every group and stopped being a participant.
    pub stopped: bool,
    /// The wait this ended: the leaver's own, or one it had filtered to the group it left.
    pub ended: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub name: String,
    pub groups: Vec<(String, Vec<Entry>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    Ready(Vec<Notice>),
    Waiting { ticket: u64, superseded: Option<u64> },
}

fn alive(participant: &Participant, presence: &dyn Presence) -> bool {
    participant.name == HUMAN || (!participant.gone && presence.alive(participant))
}

#[derive(Debug)]
struct Group {
    policy: Policy,
    members: BTreeSet<String>,
    log: Vec<Entry>,
}

impl Group {
    fn head(&self) -> u64 {
        self.log.last().map_or(0, |entry| entry.seq)
    }
}

#[derive(Debug)]
struct Waiter {
    ticket: u64,
    group: Option<String>,
}

/// The message service. Every call that changes something has been kept by the store before it
/// returns.
#[derive(Debug)]
pub struct Messaging<S: Store> {
    store: S,
    participants: BTreeMap<String, Participant>,
    groups: BTreeMap<String, Group>,
    waiters: BTreeMap<String, Waiter>,
    next_ticket: u64,
    /// What the store last took, so a request that changed nothing costs no sync.
    kept: Saved,
}

impl<S: Store> Messaging<S> {
    pub fn new(store: S) -> Messaging<S> {
        Messaging::restore(store, Saved::default(), BTreeMap::new())
    }

    /// Picks up where a previous host left off. Each group's members are read back from its
    /// log, which is appended before the saved state is written.
    pub fn restore(store: S, saved: Saved, logs: BTreeMap<String, Vec<Entry>>) -> Messaging<S> {
        let mut policies: BTreeMap<String, Policy> =
            saved.groups.into_iter().map(|record| (record.name, record.policy)).collect();
        let mut groups = BTreeMap::new();
        for (name, log) in logs {
            let mut members = BTreeSet::new();
            for entry in &log {
                match &entry.what {
                    What::Joined { who } => {
                        members.insert(who.clone());
                    }
                    What::Left { who } => {
                        members.remove(who);
                    }
                    _ => {}
                }
            }
            let policy = policies.remove(&name).unwrap_or_default();
            groups.insert(name, Group { policy, members, log });
        }
        // A cursor past its log's head is what a log that lost entries leaves; the next entry
        // would take a number the reader has already passed.
        let participants = saved
            .participants
            .into_iter()
            .map(|mut participant| {
                for (group, cursor) in &mut participant.cursors {
                    let head = groups.get(group).map_or(0, Group::head);
                    *cursor = (*cursor).min(head);
                }
                (participant.name.clone(), participant)
            })
            .collect();
        let mut messaging = Messaging {
            store,
            participants,
            groups,
            waiters: BTreeMap::new(),
            next_ticket: 1,
            kept: Saved::default(),
        };
        messaging.kept = messaging.snapshot();
        messaging
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    pub fn participant(&self, name: &str) -> Option<&Participant> {
        self.participants.get(name)
    }

    pub fn join(
        &mut self,
        caller: &Caller,
        name: Option<&str>,
        group: Option<&str>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Joined, Refusal> {
        let (name, took_over) = match name {
            None => self.identify_reporting(caller, presence)?,
            Some(name) => {
                let caller = &Self::addressed(caller, presence);
                check_participant(name)?;
                self.may_become(name, caller, presence)?;
                let took_over = self.participants.get(name).is_some_and(|existing| {
                    existing.inbox.is_some() && existing.inbox != caller.inbox
                });
                let addressed = self.made_by_address(caller).filter(|made| made != name);
                self.adopt(name, caller);
                if let Some(made) = addressed {
                    self.absorb(name, &made);
                }
                (name.to_string(), took_over)
            }
        };
        let mut created = false;
        if let Some(group) = group {
            check_group(group)?;
            created = self.ensure_group(group, &name, now_ms)?;
            self.add_member(group, &name, now_ms)?;
        }
        self.save()?;
        Ok(Joined { name, group: group.map(str::to_string), created, took_over })
    }

    pub fn leave(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Left, Refusal> {
        let caller = &Self::addressed(caller, presence);
        let name = self
            .lookup(caller)
            .filter(|name| self.participants.contains_key(name))
            .ok_or_else(|| Refusal::NotAParticipant {
                name: caller.as_name.clone().or_else(|| {
                    (caller.inbox.is_none() && caller.pane.is_none()).then(|| HUMAN.to_string())
                }),
            })?;
        let left = if let Some(group) = group {
            let members = &self.group(group)?.members;
            if !members.contains(&name) {
                return Err(Refusal::NotAMember { name, group: group.to_string() });
            }
            self.remove_member(group, &name, now_ms)?;
            let kept_to_it = self
                .waiters
                .get(&name)
                .is_some_and(|waiter| waiter.group.as_deref() == Some(group));
            let ended = if kept_to_it { self.waiters.remove(&name) } else { None };
            let ended = ended.map(|waiter| waiter.ticket);
            Left { name, groups: vec![group.to_string()], stopped: false, ended }
        } else {
            let groups = self.memberships(&name);
            for group in &groups {
                self.remove_member(group, &name, now_ms)?;
            }
            self.participants.remove(&name);
            let ended = self.waiters.remove(&name).map(|waiter| waiter.ticket);
            Left { name, groups, stopped: true, ended }
        };
        self.save()?;
        Ok(left)
    }

    pub fn post(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        to: &[String],
        body: &str,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Posted, Refusal> {
        if body.trim().is_empty() {
            return Err(Refusal::EmptyBody);
        }
        if body.len() > LARGEST_BODY {
            return Err(Refusal::BodyTooLarge { bytes: body.len() });
        }
        let author = self.identify(caller, presence)?;
        let mut addressees: Vec<String> = Vec::new();
        for name in to {
            check_participant(name)?;
            let name = &self.addressee(name, presence)?;
            if *name == author {
                return Err(Refusal::AddressedSelf);
            }
            if !addressees.contains(name) {
                addressees.push(name.clone());
            }
        }
        let group = self.resolve_group(&author, &addressees, group, now_ms)?;

        let unread = self.unread_from_others(&author, &group);
        if unread > 0 {
            return Err(Refusal::Unread { group, count: unread });
        }
        let message = What::Message {
            author: author.clone(),
            to: addressees.clone(),
            body: body.to_string(),
        };
        let seq = self.append(&group, message, now_ms)?;

        let targets: Vec<String> = if addressees.is_empty() {
            let policy = &self.groups[&group].policy;
            self.groups[&group]
                .members
                .iter()
                .filter(|member| policy.rings(&author, member))
                .cloned()
                .collect()
        } else {
            addressees
        };
        let mut posted = Posted {
            author,
            group: group.clone(),
            seq,
            reached: Vec::new(),
            wakes: Vec::new(),
            answered: Vec::new(),
            unsaved: None,
        };
        for target in targets {
            let reach = self.reach(&target, &group, &mut posted, presence);
            posted.reached.push((target, reach));
        }
        if let Err(Refusal::Store { error }) = self.save() {
            posted.unsaved = Some(error);
        }
        Ok(posted)
    }

    /// What became of the wakes a post returned: a participant whose inbox would not take one
    /// is gone until it joins again.
    pub fn delivered(&mut self, wake: &Wake, reached: bool) -> Result<(), Refusal> {
        if reached {
            return Ok(());
        }
        let Some(participant) = self.participants.get_mut(&wake.name) else {
            return Ok(());
        };
        // It may have come back from a new session while the wake was out.
        let still_there = match &wake.via {
            Via::Inbox(inbox) => participant.inbox.as_ref() == Some(inbox),
            Via::Pane(pane) => participant.pane.as_ref() == Some(pane),
        };
        if !still_there {
            return Ok(());
        }
        participant.gone = true;
        participant.woken.clear();
        self.save()
    }

    pub fn read(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Read, Refusal> {
        let name = self.identify(caller, presence)?;
        let groups = self.chosen_groups(&name, group)?;
        let before = self.participants[&name].clone();
        let mut read = Read { name: name.clone(), groups: Vec::new() };
        for group in groups {
            let cursor = self.cursor(&name, &group);
            let log = &self.groups[&group].log;
            let head = self.groups[&group].head();
            let entries: Vec<Entry> = log
                .iter()
                .filter(|entry| entry.seq > cursor)
                .filter(|entry| {
                    entry.author() != Some(name.as_str()) && entry.subject() != Some(name.as_str())
                })
                .cloned()
                .collect();
            let participant = self.participants.get_mut(&name).expect("identified");
            participant.cursors.insert(group.clone(), head);
            participant.woken.remove(&group);
            participant.rewoken.remove(&group);
            read.groups.push((group, entries));
        }
        // A cursor that moved without being kept would skip these entries after a restart,
        // and the caller is told the read failed, so it must not have moved here either.
        if let Err(refusal) = self.save() {
            self.participants.insert(name, before);
            return Err(refusal);
        }
        Ok(read)
    }

    /// The transcript, moving nothing.
    pub fn log(&self, group: &str, since: u64) -> Result<Vec<Entry>, Refusal> {
        let group = self.group(group)?;
        Ok(group.log.iter().filter(|entry| entry.seq > since).cloned().collect())
    }

    /// Returns at once when the caller has unread messages that would wake it; otherwise
    /// registers a wait that the next such post answers. A newer wait for the same participant
    /// ends the older one, so at most one runs.
    pub fn wait(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Waited, Refusal> {
        let name = self.identify(caller, presence)?;
        let groups = self.chosen_groups(&name, group)?;
        self.save()?;
        let ready: Vec<Notice> =
            groups.iter().filter_map(|group| self.notice(&name, group)).collect();
        if !ready.is_empty() {
            return Ok(Waited::Ready(ready));
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let waiter = Waiter { ticket, group: group.map(str::to_string) };
        let superseded = self.waiters.insert(name, waiter).map(|older| older.ticket);
        Ok(Waited::Waiting { ticket, superseded })
    }

    /// Forgets a wait its caller stopped waiting for.
    pub fn cancel_wait(&mut self, ticket: u64) {
        self.waiters.retain(|_, waiter| waiter.ticket != ticket);
    }

    pub fn who(
        &self,
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Vec<Member>, Refusal> {
        let names: Vec<String> = match group {
            Some(group) => self.group(group)?.members.iter().cloned().collect(),
            None => self.participants.keys().cloned().collect(),
        };
        Ok(names
            .into_iter()
            .map(|name| {
                let participant = self.participants.get(&name);
                let liveness = if name == HUMAN {
                    Liveness::Human
                } else if participant.is_some_and(|participant| alive(participant, presence)) {
                    Liveness::Alive
                } else {
                    Liveness::Gone
                };
                Member {
                    groups: self.memberships(&name),
                    inbox: participant
                        .and_then(|participant| participant.inbox.as_ref())
                        .map(|inbox| inbox.socket.clone()),
                    pane: participant.and_then(|participant| participant.pane.clone()),
                    activity: participant.and_then(|participant| presence.activity(participant)),
                    liveness,
                    name,
                }
            })
            .collect())
    }

    /// The participant `name` means in a post's `--to`: a participant by that name; else the
    /// one in the pane of that name; else, for a pane with an agent in it, a participant made
    /// for it, named after the pane, which is what lets a post reach an agent that never joined
    /// (MIP-4, section 3). The human is made on first address too.
    fn addressee(&mut self, name: &str, presence: &dyn Presence) -> Result<String, Refusal> {
        if self.participants.contains_key(name) {
            return Ok(name.to_string());
        }
        if let Some(holder) = self.by_pane(name) {
            return Ok(holder);
        }
        if name == HUMAN || presence.has_pane(name) {
            let mut participant = Participant::named(name);
            if name != HUMAN {
                participant.pane = Some(name.to_string());
            }
            self.participants.insert(name.to_string(), participant);
            return Ok(name.to_string());
        }
        Err(Refusal::NoSuchParticipant { name: name.to_string() })
    }

    // ------------------------------------------------------------------------------------------
    // Who is asking

    /// The participant the caller already is, without making one.
    fn lookup(&self, caller: &Caller) -> Option<String> {
        if let Some(name) = &caller.as_name {
            return Some(name.clone());
        }
        if let Some(name) = caller.inbox.as_ref().and_then(|inbox| self.by_inbox(inbox)) {
            return Some(name);
        }
        if let Some(name) = caller.pane.as_deref().and_then(|pane| self.by_pane(pane)) {
            return Some(name);
        }
        let agent = caller.inbox.is_some() || caller.pane.is_some();
        (!agent).then(|| HUMAN.to_string())
    }

    fn by_inbox(&self, inbox: &Inbox) -> Option<String> {
        self.participants
            .values()
            .find(|participant| participant.inbox.as_ref() == Some(inbox))
            .map(|participant| participant.name.clone())
    }

    fn by_pane(&self, pane: &str) -> Option<String> {
        self.participants
            .values()
            .find(|participant| participant.pane.as_deref() == Some(pane))
            .map(|participant| participant.name.clone())
    }

    /// The caller as far as its addresses identify it: a pane counts only while detection has
    /// found an agent in it, since a person's own shell in a pane is the human (MIP-4, section 3).
    fn addressed(caller: &Caller, presence: &dyn Presence) -> Caller {
        let mut caller = caller.clone();
        caller.pane = caller.pane.filter(|pane| presence.agent_in(pane));
        caller
    }

    /// Refuses to make the caller `name` while `name` is a live participant in another
    /// session: `join --name` and `--as` alike (MIP-4, section 3). The caller's addresses would
    /// replace the live one's, and it would never be woken again. A caller carrying no address,
    /// such as a script, moves nothing, so it may act as anyone.
    fn may_become(
        &self,
        name: &str,
        caller: &Caller,
        presence: &dyn Presence,
    ) -> Result<(), Refusal> {
        let Some(existing) = self.participants.get(name) else { return Ok(()) };
        let elsewhere = (caller.inbox.is_some() && existing.inbox != caller.inbox)
            || (caller.pane.is_some() && existing.pane.is_some() && existing.pane != caller.pane);
        if name != HUMAN && elsewhere && alive(existing, presence) {
            let inbox = existing.inbox.as_ref().map(|inbox| inbox.socket.clone());
            return Err(Refusal::NameInUse { name: name.to_string(), inbox });
        }
        Ok(())
    }

    /// The participant the caller is, registering it under a default name if it is new
    /// (MIP-4, section 3). Refreshes the addresses it carries.
    fn identify(&mut self, caller: &Caller, presence: &dyn Presence) -> Result<String, Refusal> {
        self.identify_reporting(caller, presence).map(|(name, _)| name)
    }

    /// [`Self::identify`], also saying whether the caller took over a name a gone session held.
    fn identify_reporting(
        &mut self,
        caller: &Caller,
        presence: &dyn Presence,
    ) -> Result<(String, bool), Refusal> {
        let caller = &Self::addressed(caller, presence);
        if let Some(name) = &caller.as_name {
            check_participant(name)?;
            self.may_become(name, caller, presence)?;
            self.adopt(name, caller);
            return Ok((name.clone(), false));
        }
        if let Some(name) = self.lookup(caller) {
            if name == HUMAN {
                self.participants.entry(name.clone()).or_insert_with(|| Participant::named(HUMAN));
            } else {
                self.adopt(&name, caller);
            }
            return Ok((name, false));
        }
        let base = default_name(caller.directory.as_deref());
        let name = self.free_name(&base, presence);
        let took_over = self.participants.contains_key(&name);
        self.adopt(&name, caller);
        Ok((name, took_over))
    }

    /// `base`, or `base-2`, `base-3` and on: the first that nobody alive holds. A default name
    /// held by a participant that is gone is taken over, which is what brings a resumed session
    /// in the same directory back under its old name.
    fn free_name(&self, base: &str, presence: &dyn Presence) -> String {
        let mut name = base.to_string();
        let mut suffix = 1;
        while self.participants.get(&name).is_some_and(|holder| alive(holder, presence)) {
            suffix += 1;
            name = format!("{base}-{suffix}");
        }
        name
    }

    /// Makes `name` the participant the caller's addresses belong to, creating it if needed. An
    /// address belongs to one participant, so any other holding it lets go, and one that is left
    /// holding nothing - a default name the caller has since replaced with its own - goes.
    fn adopt(&mut self, name: &str, caller: &Caller) {
        let participant =
            self.participants.entry(name.to_string()).or_insert_with(|| Participant::named(name));
        let moved = replaced(participant.inbox.as_ref(), caller.inbox.as_ref())
            || replaced(participant.pane.as_ref(), caller.pane.as_ref());
        // A session that takes over a name has been woken for nothing yet, whatever the session
        // before it was told. One that only adds an address - the agent in a pane that was
        // addressed by the pane's name, running its first command - is the one that was woken.
        if moved {
            participant.woken.clear();
            participant.rewoken.clear();
        }
        if caller.inbox.is_some() {
            participant.inbox.clone_from(&caller.inbox);
            participant.gone = false;
        }
        if caller.pane.is_some() {
            participant.pane.clone_from(&caller.pane);
            participant.gone = false;
        }
        let others: Vec<String> = self
            .participants
            .values()
            .filter(|other| other.name != name)
            .filter(|other| {
                (caller.inbox.is_some() && other.inbox == caller.inbox)
                    || (caller.pane.is_some() && other.pane == caller.pane)
            })
            .map(|other| other.name.clone())
            .collect();
        for other in others {
            if self.memberships(&other).is_empty() {
                self.participants.remove(&other);
                continue;
            }
            let Some(other) = self.participants.get_mut(&other) else { continue };
            if caller.inbox.is_some() && other.inbox == caller.inbox {
                other.inbox = None;
            }
            if caller.pane.is_some() && other.pane == caller.pane {
                other.pane = None;
            }
        }
    }

    /// The participant a post made by addressing the caller's pane (MIP-4, section 3): named
    /// after the pane, holding it, and with no inbox of its own yet.
    fn made_by_address(&self, caller: &Caller) -> Option<String> {
        let pane = caller.pane.as_ref()?;
        let made = self.participants.get(pane)?;
        (made.pane.as_ref() == Some(pane) && made.inbox.is_none()).then(|| pane.clone())
    }

    /// Folds `from` into `into`: its groups, where it had read to, and what it was woken for.
    /// An agent addressed by its pane that then joins under a name of its own is one agent, and
    /// the brief it was rung for must be readable under that name.
    fn absorb(&mut self, into: &str, from: &str) {
        let Some(from) = self.participants.remove(from) else { return };
        for group in self.groups.values_mut() {
            if group.members.remove(&from.name) {
                group.members.insert(into.to_string());
            }
        }
        self.waiters.remove(&from.name);
        let Some(into) = self.participants.get_mut(into) else { return };
        for (group, cursor) in from.cursors {
            into.cursors.entry(group).or_insert(cursor);
        }
        into.woken.extend(from.woken);
        into.rewoken.extend(from.rewoken);
    }

    // ------------------------------------------------------------------------------------------
    // Groups

    fn group(&self, name: &str) -> Result<&Group, Refusal> {
        self.groups.get(name).ok_or_else(|| Refusal::NoSuchGroup { group: name.to_string() })
    }

    fn memberships(&self, name: &str) -> Vec<String> {
        self.groups
            .iter()
            .filter(|(_, group)| group.members.contains(name))
            .map(|(group, _)| group.clone())
            .collect()
    }

    /// The one group named, which the caller must be in, or every group it is in.
    fn chosen_groups(&self, name: &str, group: Option<&str>) -> Result<Vec<String>, Refusal> {
        match group {
            Some(group) => {
                if !self.group(group)?.members.contains(name) {
                    return Err(Refusal::NotAMember {
                        name: name.to_string(),
                        group: group.to_string(),
                    });
                }
                Ok(vec![group.to_string()])
            }
            None => Ok(self.memberships(name)),
        }
    }

    /// Creates the group with the default policy if it does not exist, saying whether it did.
    fn ensure_group(&mut self, name: &str, by: &str, now_ms: u64) -> Result<bool, Refusal> {
        if self.groups.contains_key(name) {
            return Ok(false);
        }
        if let Some(existing) = self.groups.keys().find(|other| other.eq_ignore_ascii_case(name)) {
            return Err(Refusal::GroupNameClash {
                group: name.to_string(),
                existing: existing.clone(),
            });
        }
        let group = Group { policy: Policy::default(), members: BTreeSet::new(), log: Vec::new() };
        self.groups.insert(name.to_string(), group);
        if let Err(refusal) = self.append(name, What::Created { by: by.to_string() }, now_ms) {
            self.groups.remove(name);
            return Err(refusal);
        }
        Ok(true)
    }

    /// Adds a member, whose reading starts from now: history from before it joined is what
    /// `log` is for, and counting it as unread would refuse the newcomer's first post.
    fn add_member(&mut self, group: &str, name: &str, now_ms: u64) -> Result<(), Refusal> {
        if self.groups[group].members.contains(name) {
            return Ok(());
        }
        let seq = self.append(group, What::Joined { who: name.to_string() }, now_ms)?;
        if let Some(participant) = self.participants.get_mut(name) {
            participant.cursors.insert(group.to_string(), seq);
            participant.woken.remove(group);
        }
        Ok(())
    }

    fn remove_member(&mut self, group: &str, name: &str, now_ms: u64) -> Result<(), Refusal> {
        self.append(group, What::Left { who: name.to_string() }, now_ms)?;
        if let Some(participant) = self.participants.get_mut(name) {
            participant.cursors.remove(group);
            participant.woken.remove(group);
        }
        Ok(())
    }

    /// Which group a post goes to (MIP-4, section 2).
    fn resolve_group(
        &mut self,
        author: &str,
        addressees: &[String],
        group: Option<&str>,
        now_ms: u64,
    ) -> Result<String, Refusal> {
        if let Some(group) = group {
            check_group(group)?;
            let members = &self.group(group)?.members;
            if !members.contains(author) {
                return Err(Refusal::NotAMember {
                    name: author.to_string(),
                    group: group.to_string(),
                });
            }
            if let Some(outside) = addressees.iter().find(|name| !members.contains(*name)) {
                return Err(Refusal::AddresseeNotInGroup {
                    name: outside.clone(),
                    group: group.to_string(),
                });
            }
            return Ok(group.to_string());
        }
        let mine = self.memberships(author);
        if addressees.is_empty() {
            return match mine.as_slice() {
                [] => Err(Refusal::NoGroup),
                [only] => Ok(only.clone()),
                _ => Err(Refusal::WhichGroup { candidates: mine }),
            };
        }
        let shared: Vec<String> = mine
            .into_iter()
            .filter(|group| {
                let members = &self.groups[group].members;
                addressees.iter().all(|name| members.contains(name))
            })
            .collect();
        match shared.as_slice() {
            [only] => Ok(only.clone()),
            [] => {
                let mut everyone: Vec<&str> = vec![author];
                everyone.extend(addressees.iter().map(String::as_str));
                let group = pair_group(&everyone);
                if check_group(&group).is_err() {
                    return Err(Refusal::PairTooLong { group });
                }
                self.ensure_group(&group, author, now_ms)?;
                for name in everyone {
                    self.add_member(&group, name, now_ms)?;
                }
                Ok(group)
            }
            _ => Err(Refusal::WhichGroup { candidates: shared }),
        }
    }

    // ------------------------------------------------------------------------------------------
    // The log, cursors and wakes

    fn append(&mut self, group: &str, what: What, now_ms: u64) -> Result<u64, Refusal> {
        let seq = self.groups[group].head() + 1;
        let entry = Entry { seq, at_ms: now_ms, what };
        self.store.append(group, &entry).map_err(|error| Refusal::Store { error })?;
        let group = self.groups.get_mut(group).expect("appending to a group that exists");
        match &entry.what {
            What::Joined { who } => {
                group.members.insert(who.clone());
            }
            What::Left { who } => {
                group.members.remove(who);
            }
            _ => {}
        }
        group.log.push(entry);
        Ok(seq)
    }

    fn cursor(&self, name: &str, group: &str) -> u64 {
        self.participants
            .get(name)
            .and_then(|participant| participant.cursors.get(group))
            .copied()
            .unwrap_or(0)
    }

    /// Messages by others after the cursor: what the guard counts. Notices never count, or
    /// every join would make every pending post stale, as it did in council v1.
    fn unread_from_others(&self, name: &str, group: &str) -> u64 {
        let cursor = self.cursor(name, group);
        let count = self.groups[group]
            .log
            .iter()
            .filter(|entry| entry.seq > cursor)
            .filter(|entry| entry.author().is_some_and(|author| author != name))
            .count();
        count as u64
    }

    /// What to tell `name` about `group`: its unread messages that would wake it, or nothing
    /// when there are none.
    fn notice(&self, name: &str, group: &str) -> Option<Notice> {
        let cursor = self.cursor(name, group);
        let policy = &self.groups[group].policy;
        let mut notice: Option<Notice> = None;
        for entry in self.groups[group].log.iter().filter(|entry| entry.seq > cursor) {
            let What::Message { author, to, .. } = &entry.what else { continue };
            let addressed = to.iter().any(|addressee| addressee == name);
            let wakes = if to.is_empty() { policy.rings(author, name) } else { addressed };
            if author == name || !wakes {
                continue;
            }
            let notice = notice.get_or_insert_with(|| Notice {
                group: group.to_string(),
                first: entry.seq,
                last: entry.seq,
                count: 0,
                to_you: 0,
                from: Vec::new(),
                again: false,
            });
            notice.last = entry.seq;
            notice.count += 1;
            notice.to_you += u64::from(addressed);
            if !notice.from.contains(author) {
                notice.from.push(author.clone());
            }
        }
        notice
    }

    /// Wakes `name` for `group` if nothing has since the last time it read.
    fn reach(
        &mut self,
        name: &str,
        group: &str,
        posted: &mut Posted,
        presence: &dyn Presence,
    ) -> Reach {
        let Some(participant) = self.participants.get(name) else {
            return Reach::Gone;
        };
        if participant.gone {
            return Reach::Gone;
        }
        let Some(notice) = self.notice(name, group) else {
            return Reach::Waiting;
        };
        let waiting = self
            .waiters
            .get(name)
            .is_some_and(|waiter| waiter.group.as_deref().is_none_or(|filter| filter == group));
        let participant = self.participants.get_mut(name).expect("looked up above");
        if waiting {
            let waiter = self.waiters.remove(name).expect("looked up above");
            participant.woken.insert(group.to_string());
            posted.answered.push(AnsweredWait {
                ticket: waiter.ticket,
                name: name.to_string(),
                notice,
            });
            return Reach::Woken;
        }
        // Nothing wakes the human's window until attention routing does (MIP-4, section 10);
        // a wait of the human's is answered above like anyone's.
        if name == HUMAN {
            return Reach::Waiting;
        }
        let via = Self::via(participant, presence);
        // Woken already only while something is there to take this too: an agent whose pane
        // closed, or who left it, was woken for nothing it will read (MIP-4, section 7).
        let to_come = participant
            .pane
            .as_ref()
            .is_some_and(|pane| presence.doorbell(pane) == Ringable::AgentToCome);
        if participant.woken.contains(group)
            && via.is_ok()
            && (to_come || presence.alive(participant))
        {
            return Reach::AlreadyWoken;
        }
        let via = match via {
            Ok(via) => via,
            Err(reach) => return reach,
        };
        let participant = self.participants.get_mut(name).expect("looked up above");
        participant.woken.insert(group.to_string());
        posted.wakes.push(Wake { name: name.to_string(), via, notice });
        Reach::Woken
    }

    /// How to wake a participant. An agent in a pane is rung there even when it has an inbox
    /// too: a session that bypasses permission prompts holds an inbox message for a person's
    /// approval (`docs/observations/claude-code-2.1.283.md`), the host cannot tell which
    /// sessions do, and the doorbell's guards make typing into a pane safe (MIP-4, section 6).
    ///
    /// When nothing can, says how the post reached it instead: a pane with no agent, one whose
    /// agent the doorbell cannot read, or nothing at all, so it reads the message when it next
    /// reads.
    fn via(participant: &Participant, presence: &dyn Presence) -> Result<Via, Reach> {
        let doorbell = participant.pane.as_ref().map(|pane| (pane, presence.doorbell(pane)));
        if let Some((pane, doorbell)) = doorbell
            && doorbell.rings()
        {
            return Ok(Via::Pane(pane.clone()));
        }
        if let Some(inbox) = &participant.inbox {
            return Ok(Via::Inbox(inbox.clone()));
        }
        Err(match doorbell {
            Some((_, Ringable::NoPrompt)) => Reach::NoDoorbell,
            Some(_) => Reach::NoAgent,
            None => Reach::Waiting,
        })
    }

    /// A wake for every group an agent in a pane was woken for and has not read: what a host
    /// that is starting cannot tell was rung before it stopped, so it rings them again.
    pub fn outstanding(&self) -> Vec<Wake> {
        let mut wakes = Vec::new();
        for participant in self.participants.values() {
            let Some(pane) = &participant.pane else { continue };
            for group in &participant.woken {
                if let Some(notice) = self.notice(&participant.name, group) {
                    let via = Via::Pane(pane.clone());
                    wakes.push(Wake { name: participant.name.clone(), via, notice });
                }
            }
        }
        wakes
    }

    /// Forgets that `name` was woken for `group`: the wake never reached it, so the next post
    /// there wakes it afresh rather than finding it already woken. Returns why the state could
    /// not be saved, when it could not.
    pub fn unwake(&mut self, name: &str, group: &str) -> Option<String> {
        let participant = self.participants.get_mut(name)?;
        participant.woken.remove(group);
        participant.rewoken.remove(group);
        match self.save() {
            Err(Refusal::Store { error }) => Some(error),
            _ => None,
        }
    }

    /// Whether `name` was woken for `group` and has not read it since.
    pub fn woken_for(&self, name: &str, group: &str) -> bool {
        self.participants.get(name).is_some_and(|participant| participant.woken.contains(group))
    }

    /// Participants woken for a group they have not read since, which have not yet been woken
    /// a second time for it, with the pane each is in: whose going idle the host watches for.
    pub fn watched(&self) -> Vec<(String, String)> {
        self.participants
            .values()
            .filter(|participant| {
                participant.woken.iter().any(|group| !participant.rewoken.contains(group))
            })
            .filter_map(|participant| {
                participant.pane.clone().map(|pane| (participant.name.clone(), pane))
            })
            .collect()
    }

    /// `name`'s agent went idle. For each group it was woken for and has still not read, wake
    /// it once more, saying so, and never again until it reads (MIP-4, section 5): the first
    /// wake may have landed while it was busy with something it then finished.
    ///
    /// Also returns why the state could not be saved, when it could not: the wakes stand, and
    /// what is at risk is a second "still unread" after a restart.
    pub fn went_idle(
        &mut self,
        name: &str,
        presence: &dyn Presence,
    ) -> (Vec<Wake>, Option<String>) {
        let Some(participant) = self.participants.get(name) else { return (Vec::new(), None) };
        let Ok(via) = Self::via(participant, presence) else { return (Vec::new(), None) };
        let groups: Vec<String> = participant
            .woken
            .iter()
            .filter(|group| !participant.rewoken.contains(*group))
            .cloned()
            .collect();
        let mut wakes = Vec::new();
        for group in groups {
            let Some(mut notice) = self.notice(name, &group) else { continue };
            notice.again = true;
            if let Some(participant) = self.participants.get_mut(name) {
                participant.rewoken.insert(group);
            }
            wakes.push(Wake { name: name.to_string(), via: via.clone(), notice });
        }
        let unsaved = match self.save() {
            Err(Refusal::Store { error }) => Some(error),
            _ => None,
        };
        (wakes, unsaved)
    }

    fn save(&mut self) -> Result<(), Refusal> {
        let saved = self.snapshot();
        if saved == self.kept {
            return Ok(());
        }
        self.store.save(&saved).map_err(|error| Refusal::Store { error })?;
        self.kept = saved;
        Ok(())
    }

    fn snapshot(&self) -> Saved {
        Saved {
            participants: self.participants.values().cloned().collect(),
            groups: self
                .groups
                .iter()
                .map(|(name, group)| GroupRecord {
                    name: name.clone(),
                    policy: group.policy.clone(),
                })
                .collect(),
        }
    }
}

/// Whether an address a participant holds is being replaced by another, rather than given for
/// the first time or given again.
fn replaced<T: PartialEq>(held: Option<&T>, given: Option<&T>) -> bool {
    held.is_some() && given.is_some() && held != given
}
