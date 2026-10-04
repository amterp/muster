use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::across::{HumanHome, Peer, Tell};
use crate::names::{check_addressee, check_group, check_participant, is_human, split_machine};
use crate::{
    Action, Change, Entry, GroupRecord, HUMAN, LARGEST_BODY, Policy, Refusal, Saved, Store, What,
    default_name, pair_group,
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
    /// When the host took the request, in milliseconds since the Unix epoch: how the service
    /// knows a participant's hooks have run lately.
    pub at_ms: u64,
}

/// How long after a participant's hooks last ran they still count as running: a turn's tool
/// calls come further apart than this only in a turn stuck on one, which its `Stop` hook ends.
const HOOKS_LIVE_MS: u64 = 5 * 60 * 1000;

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
    /// Its hooks fetch its messages (MIP-4, section 6): a `PostToolUse` hook reads between tool
    /// calls, and a `Stop` hook waits with `--due` once a turn ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pull: bool,
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
            pull: false,
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

    /// Whether a window is attending, which is what wakes the human (MIP-4, section 10).
    fn attended(&self) -> bool {
        false
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
    /// On another machine this one cannot reach now, or could not ask.
    Unreachable,
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

/// A message as its author sends it: what `post` is asked.
#[derive(Debug, Clone, Copy, Default)]
pub struct Draft<'a> {
    /// The group to post to; none for the one its author and addressees share.
    pub group: Option<&'a str>,
    pub to: &'a [String],
    /// `name@machine` for each name in `to` that means nobody here and another machine said it
    /// has (MIP-4, section 11).
    pub found: &'a [String],
    pub body: &'a str,
    /// To reach its addressees mid-turn, even those woken already, rather than once they are
    /// idle.
    pub urgent: bool,
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
    /// How many of them were posted urgently.
    pub urgent: u64,
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
    /// Attention routing, for the human: the host tells the windows attending it what waits
    /// for the human, which raise a notification (MIP-4, section 10).
    Human,
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
    /// The group is paused: woken once it is resumed (MIP-4, section 8).
    Paused,
    /// On another machine, which could not be told.
    Unreachable,
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
    /// Other machines with members in the group, which the host sends the new entry to.
    pub tell: Vec<Tell>,
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
    pub tell: Vec<Tell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub name: String,
    pub groups: Vec<String>,
    /// Left every group and stopped being a participant.
    pub stopped: bool,
    /// The wait this ended: the leaver's own, or one it had filtered to the group it left.
    pub ended: Option<u64>,
    pub tell: Vec<Tell>,
}

/// A change to a group: made, its policy set, members added or removed, paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub by: String,
    pub group: String,
    /// The notice logged for it; none when it changed nothing.
    pub seq: Option<u64>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Waits a removal ended.
    pub ended: Vec<u64>,
    /// Other machines with members in the group, which the host sends the change to.
    pub tell: Vec<Tell>,
}

/// A group deleted, and what the host still has to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub by: String,
    pub group: String,
    /// How many entries its log held.
    pub entries: u64,
    /// Its members when it went, this machine's and other machines'.
    pub let_go: Vec<String>,
    /// Waits kept to the group, which the host ends as a leave does.
    pub ended: Vec<u64>,
    /// Other machines with members in it, whose replicas the host tells to go.
    pub forget: Vec<String>,
    /// Why the state could not be saved afterwards. The group is deleted all the same: its log
    /// is gone, and a restore drops what the state still holds of a group with no log.
    pub unsaved: Option<String>,
}

/// A replica forgotten because its home deleted the group, or no longer has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forgot {
    /// The replica's name here.
    pub group: String,
    pub ended: Vec<u64>,
    /// Nothing waits for the human there now, if anything did.
    pub wakes: Vec<Wake>,
    pub unsaved: Option<String>,
}

/// A group as `groups` lists it.
impl Changed {
    fn by(by: &str, group: &str, seq: Option<u64>) -> Changed {
        Changed {
            by: by.to_string(),
            group: group.to_string(),
            seq,
            added: Vec::new(),
            removed: Vec::new(),
            ended: Vec::new(),
            tell: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSummary {
    pub name: String,
    pub members: Vec<String>,
    pub policy: Policy,
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

pub(crate) fn alive(participant: &Participant, presence: &dyn Presence) -> bool {
    participant.name == HUMAN || (!participant.gone && presence.alive(participant))
}

#[derive(Debug)]
pub(crate) struct Group {
    pub(crate) policy: Policy,
    pub(crate) members: BTreeSet<String>,
    pub(crate) log: Vec<Entry>,
    /// The machine the group is kept on, when that is not this one: this is its replica, which
    /// the store never sees (MIP-4, sections 11 and 12).
    pub(crate) home: Option<String>,
}

impl Group {
    /// A replica of a group kept on `machine` that holds nothing yet, until its home sends it.
    pub(crate) fn replica(machine: &str) -> Group {
        Group {
            policy: Policy::default(),
            members: BTreeSet::new(),
            log: Vec::new(),
            home: Some(machine.to_string()),
        }
    }

    pub(crate) fn head(&self) -> u64 {
        self.log.last().map_or(0, |entry| entry.seq)
    }
}

#[derive(Debug)]
pub(crate) struct Waiter {
    pub(crate) ticket: u64,
    pub(crate) group: Option<String>,
    /// Answered only with a wake that is due, never a second one for the same batch: a `Stop`
    /// hook's wait, whose answer starts a turn.
    pub(crate) due: bool,
}

/// The message service. Every call that changes something has been kept by the store before it
/// returns.
#[derive(Debug)]
pub struct Messaging<S: Store> {
    pub(crate) store: S,
    pub(crate) participants: BTreeMap<String, Participant>,
    /// Groups kept here by name, and replicas of groups kept elsewhere as `name@machine`.
    pub(crate) groups: BTreeMap<String, Group>,
    pub(crate) waiters: BTreeMap<String, Waiter>,
    /// When each participant last ran a verb, in the host's milliseconds. Not kept: a host that
    /// starts has seen nobody's hooks run.
    seen_ms: BTreeMap<String, u64>,
    next_ticket: u64,
    /// What the store last took, so a request that changed nothing costs no sync.
    kept: Saved,
    /// Machines a link to is up now.
    pub(crate) linked: BTreeSet<String>,
    /// Every machine a link has come up to since this daemon started, whether or not it is up
    /// now: those that may keep a group by a name nothing here holds.
    pub(crate) met: BTreeSet<String>,
    /// What those machines call this one.
    pub(crate) called: BTreeSet<String>,
    /// Replicas a change to went unanswered, until their home is next heard from.
    pub(crate) unanswered: BTreeSet<String>,
    /// Where the human is homed, once a daemon there has dialed this one.
    pub(crate) human_home: Option<HumanHome>,
    /// Whom each replica's last batch of new entries reached, for a later batch carrying the
    /// same entries (`across::Lately`). Not kept: it answers a race between two deliveries.
    pub(crate) lately_reached: BTreeMap<String, crate::across::Lately>,
}

impl<S: Store> Messaging<S> {
    pub fn new(store: S) -> Messaging<S> {
        Messaging::restore(store, Saved::default(), BTreeMap::new())
    }

    /// Picks up where a previous host left off. Each group's members are read back from its
    /// log, which is appended before the saved state is written - and so are its policy and
    /// whether it is paused, where the log records them, since the saved state may have missed
    /// its last write (`muster-daemon`'s store does not sync the directory after replacing it, so
    /// a power loss can undo the last replace).
    pub fn restore(store: S, saved: Saved, logs: BTreeMap<String, Vec<Entry>>) -> Messaging<S> {
        let mut policies: BTreeMap<String, Policy> =
            saved.groups.into_iter().map(|record| (record.name, record.policy)).collect();
        let mut groups = BTreeMap::new();
        for (name, log) in logs {
            let mut members = BTreeSet::new();
            let mut set: Option<Policy> = None;
            let mut paused: Option<bool> = None;
            for entry in &log {
                match &entry.what {
                    What::Joined { who } => {
                        members.insert(who.clone());
                    }
                    What::Left { who } => {
                        members.remove(who);
                    }
                    What::Changed { change: Change::SetPolicy, policy: Some(policy), .. } => {
                        set = Some((**policy).clone());
                    }
                    What::Changed { change: Change::Paused, .. } => paused = Some(true),
                    What::Changed { change: Change::Resumed, .. } => paused = Some(false),
                    _ => {}
                }
            }
            let mut policy = policies.remove(&name).unwrap_or_default();
            if let Some(set) = set {
                policy = Policy { paused: policy.paused, ..set };
            }
            if let Some(paused) = paused {
                policy.paused = paused;
            }
            groups.insert(name, Group { policy, members, log, home: None });
        }
        // A cursor past its log's head is what a log that lost entries leaves; the next entry
        // would take a number the reader has already passed. A replica's log is not kept, so its
        // cursors wait for the refetch instead, and the replica is held empty until then: the
        // group answers as there, and behind, rather than as never heard of (MIP-4, section 12).
        // Its members here are those with a cursor on it, which leaving takes away. Being woken
        // for it is forgotten, since the refetch wakes each member afresh for what it has unread.
        let participants = saved
            .participants
            .into_iter()
            .map(|mut participant| {
                for (group, cursor) in &mut participant.cursors {
                    if let Some((_, machine)) = split_machine(group) {
                        groups
                            .entry(group.clone())
                            .or_insert_with(|| Group::replica(machine))
                            .members
                            .insert(participant.name.clone());
                        continue;
                    }
                    let head = groups.get(group).map_or(0, Group::head);
                    *cursor = (*cursor).min(head);
                }
                // A group deleted just before a crash lost its log and not yet its cursors.
                participant.cursors.retain(|group, _| {
                    split_machine(group).is_some() || groups.contains_key(group)
                });
                participant
                    .woken
                    .retain(|group| groups.get(group).is_some_and(|g| g.home.is_none()));
                participant
                    .rewoken
                    .retain(|group| groups.get(group).is_some_and(|g| g.home.is_none()));
                (participant.name.clone(), participant)
            })
            .collect();
        let mut messaging = Messaging {
            store,
            participants,
            groups,
            waiters: BTreeMap::new(),
            seen_ms: BTreeMap::new(),
            next_ticket: 1,
            kept: Saved::default(),
            linked: BTreeSet::new(),
            met: BTreeSet::new(),
            called: BTreeSet::new(),
            unanswered: BTreeSet::new(),
            lately_reached: BTreeMap::new(),
            human_home: saved.human_home,
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
            Some(name) => (name.to_string(), self.become_named(caller, name, presence)?),
        };
        self.seen(&name, caller);
        let mut created = false;
        let mut tell = Vec::new();
        if let Some(group) = group {
            check_group(group)?;
            if let Some(existing) = self.groups.get(group)
                && !existing.members.contains(&name)
            {
                self.permitted(group, &name, Action::Join)?;
            }
            created = self.ensure_group(group, &name, now_ms)?;
            let after = self.groups[group].head();
            self.add_member(group, &name, now_ms)?;
            tell = self.tell(group, after, None);
        }
        self.save()?;
        Ok(Joined { name, group: group.map(str::to_string), created, took_over, tell })
    }

    /// Makes the caller the participant `name`, as `join --name` does, saying whether it took
    /// the name over from a session that has gone.
    pub(crate) fn become_named(
        &mut self,
        caller: &Caller,
        name: &str,
        presence: &dyn Presence,
    ) -> Result<bool, Refusal> {
        let caller = &Self::addressed(caller, presence);
        check_participant(name)?;
        if name == HUMAN {
            self.human_here(presence)?;
        }
        self.may_become(name, caller, presence)?;
        let took_over = self
            .participants
            .get(name)
            .is_some_and(|existing| existing.inbox.is_some() && existing.inbox != caller.inbox);
        let addressed = self.made_by_address(caller).filter(|made| made != name);
        self.adopt(name, caller);
        if let Some(made) = addressed {
            self.absorb(name, &made);
        }
        Ok(took_over)
    }

    pub fn leave(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Left, Refusal> {
        let caller = &Self::addressed(caller, presence);
        if self.lookup(caller).as_deref() == Some(HUMAN) {
            self.human_here(presence)?;
        }
        let name = self
            .lookup(caller)
            .filter(|name| self.participants.contains_key(name))
            .ok_or_else(|| Refusal::NotAParticipant {
                name: caller.as_name.clone().or_else(|| {
                    (caller.inbox.is_none() && caller.pane.is_none()).then(|| HUMAN.to_string())
                }),
            })?;
        let left = if let Some(group) = group {
            let group = &self.locate(group)?;
            if !self.groups[group].members.contains(&name) {
                return Err(Refusal::NotAMember { name, group: group.clone() });
            }
            self.here(group)?;
            self.permitted(group, &name, Action::Leave)?;
            let after = self.groups[group].head();
            self.remove_member(group, &name, now_ms)?;
            let kept_to_it = self
                .waiters
                .get(&name)
                .is_some_and(|waiter| waiter.group.as_deref() == Some(group.as_str()));
            let ended = if kept_to_it { self.waiters.remove(&name) } else { None };
            let ended = ended.map(|waiter| waiter.ticket);
            let tell = self.tell(group, after, None);
            Left { name, groups: vec![group.clone()], stopped: false, ended, tell }
        } else {
            // Groups kept elsewhere are left through their homes first (`route_leave`).
            let groups: Vec<String> = self
                .memberships(&name)
                .into_iter()
                .filter(|group| self.groups[group].home.is_none())
                .collect();
            let mut tell = Vec::new();
            for group in &groups {
                self.permitted(group, &name, Action::Leave)?;
            }
            for group in &groups {
                let after = self.groups[group].head();
                self.remove_member(group, &name, now_ms)?;
                tell.extend(self.tell(group, after, None));
            }
            self.participants.remove(&name);
            let ended = self.waiters.remove(&name).map(|waiter| waiter.ticket);
            Left { name, groups, stopped: true, ended, tell }
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
        let draft = Draft { group, to, body, ..Draft::default() };
        self.post_draft(caller, &draft, presence, now_ms)
    }

    /// [`Self::post`], urgent or not, and with names other machines said they have.
    pub fn post_draft(
        &mut self,
        caller: &Caller,
        draft: &Draft<'_>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Posted, Refusal> {
        check_body(draft.body)?;
        let author = self.acting(caller, presence)?;
        let group = draft.group.map(|group| self.locate(group)).transpose()?;
        let addressees =
            self.addressees(&author, draft.to, draft.found, group.as_deref(), presence)?;
        let group = self.resolve_group(&author, &addressees, group.as_deref(), now_ms)?;
        self.here(&group)?;
        let cursor = self.cursor(&author, &group);
        let body = draft.body;
        let urgent = draft.urgent;
        self.post_as(&author, &group, addressees, body, urgent, cursor, None, presence, now_ms)
    }

    /// Appends a post by `author`, who had read `group` up to `cursor`, and reaches whoever it
    /// is for on this machine. The author's own daemon is where its cursor lives, so a post
    /// forwarded from another machine brings the cursor with it (MIP-4, section 4). The
    /// group's policy binds here, where it is kept, whichever machine the post came from. That
    /// machine, `from`, learns of the entry from its answer, so it is not told it again.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post_as(
        &mut self,
        author: &str,
        group: &str,
        addressees: Vec<String>,
        body: &str,
        urgent: bool,
        cursor: u64,
        from: Option<&str>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Posted, Refusal> {
        let policy = &self.groups[group].policy;
        if urgent && !policy.urges(author) {
            return Err(Refusal::NotUrgent {
                group: group.to_string(),
                urgent: policy.urgent.clone(),
            });
        }
        if let Some(addressee) = addressees.iter().find(|name| !policy.allows(author, name)) {
            return Err(Refusal::NotAllowed {
                addressee: addressee.clone(),
                group: group.to_string(),
                allowed: policy.allowed(author),
            });
        }
        // The guard keeps a model from acting on context that has gone stale. The human reads
        // the transcript as it arrives, and the daemon cannot see a person's screen, so it has
        // no cursor worth holding a person's post to (MIP-4, section 4) - on any machine.
        let unread = if is_human(author) { 0 } else { self.unread_after(author, group, cursor) };
        if unread > 0 {
            return Err(Refusal::Unread { group: group.to_string(), count: unread });
        }
        let message = What::Message {
            author: author.to_string(),
            to: addressees.clone(),
            body: body.to_string(),
            urgent,
        };
        let seq = self.append(group, message, now_ms)?;
        let targets = self.targets(group, &self.groups[group].policy, author, addressees);
        let mut posted = Posted {
            author: author.to_string(),
            group: group.to_string(),
            seq,
            reached: Vec::new(),
            wakes: Vec::new(),
            answered: Vec::new(),
            tell: self.tell(group, seq - 1, from),
            unsaved: None,
        };
        for tell in &mut posted.tell {
            tell.targets = targets
                .iter()
                .filter(|target| {
                    split_machine(target).is_some_and(|(_, machine)| machine == tell.machine)
                })
                .cloned()
                .collect();
        }
        for target in targets {
            // A member on another machine is reached by its own daemon (MIP-4, section 11).
            if split_machine(&target).is_some() {
                continue;
            }
            let reach = self.reach_member(&target, group, urgent, &mut posted, presence, now_ms);
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
            Via::Human => false,
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
            // Never back: a replica held empty after a restart is behind what was read of it.
            participant.cursors.insert(group.clone(), head.max(cursor));
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
        let group = &self.groups[&self.locate(group)?];
        Ok(group.log.iter().filter(|entry| entry.seq > since).cloned().collect())
    }

    /// Returns at once when the caller has unread messages that would wake it; otherwise
    /// registers a wait that the next such post answers. A newer wait for the same participant
    /// ends the older one, so at most one runs.
    /// Answers at once if the caller has waking messages unread, and otherwise waits for one.
    ///
    /// With `due`, which is how a `Stop` hook waits (MIP-4, section 6), it answers only with a
    /// wake the caller is due under section 5: once per batch, and once more "still unread",
    /// then nothing until it reads. It counts as a wake, and marks the caller as fetching its
    /// messages with its hooks.
    pub fn wait(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        due: bool,
        presence: &dyn Presence,
    ) -> Result<Waited, Refusal> {
        let name = self.identify(caller, presence)?;
        let groups = self.chosen_groups(&name, group)?;
        if due && let Some(participant) = self.participants.get_mut(&name) {
            participant.pull = true;
        }
        let mut ready: Vec<Notice> = Vec::new();
        for group in &groups {
            let Some(mut notice) = self.notice(&name, group) else { continue };
            if due {
                let participant = self.participants.get_mut(&name).expect("identified");
                if !participant.woken.contains(group) {
                    participant.woken.insert(group.clone());
                } else if !participant.rewoken.contains(group) {
                    participant.rewoken.insert(group.clone());
                    notice.again = true;
                } else {
                    continue;
                }
            }
            ready.push(notice);
        }
        self.save()?;
        if !ready.is_empty() {
            return Ok(Waited::Ready(ready));
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let waiter = Waiter { ticket, group: group.map(str::to_string), due };
        let superseded = self.waiters.insert(name, waiter).map(|older| older.ticket);
        Ok(Waited::Waiting { ticket, superseded })
    }

    /// Marks `name` as fetching its messages with its hooks: `join --pull`.
    pub fn pulls(&mut self, name: &str) -> Result<(), Refusal> {
        if let Some(participant) = self.participants.get_mut(name) {
            participant.pull = true;
        }
        self.save()
    }

    /// Whether `name`'s hooks will fetch what it is sent: it fetches with hooks, and a wait of
    /// its own is connected, or it ran a verb lately and is not idle (MIP-4, section 7). While
    /// they will, the host rings nothing for it.
    ///
    /// Between turns only a `Stop` hook's wait fetches. An agent idle with none connected ended
    /// its turn without one - an API error, or a person pressing Esc - and is rung as any other.
    /// An agent whose activity is unknown, with no pane to show it, is taken at its last verb.
    pub fn hooked(&self, name: &str, now_ms: u64, presence: &dyn Presence) -> bool {
        let Some(participant) = self.participants.get(name).filter(|found| found.pull) else {
            return false;
        };
        if self.waiters.contains_key(name) {
            return true;
        }
        let lately =
            self.seen_ms.get(name).is_some_and(|seen| now_ms.saturating_sub(*seen) < HOOKS_LIVE_MS);
        let idle =
            matches!(presence.activity(participant), Some(Activity::Idle | Activity::Waiting));
        lately && !idle
    }

    /// Whether `name` has said its hooks fetch its messages: `join --pull`, or a `wait --due`.
    pub fn fetches_with_hooks(&self, name: &str) -> bool {
        self.participants.get(name).is_some_and(|participant| participant.pull)
    }

    /// Answers `name`'s wait with a wake the host was holding for its pane, when the wait fits
    /// it: a `Stop` hook that connected after its agent was seen idle is told there, and
    /// nothing is typed. `None` when no such wait is connected, or nothing is unread any more.
    pub fn hand_to_wait(&mut self, name: &str, wake: &Notice) -> Option<AnsweredWait> {
        let fits = self
            .waiters
            .get(name)
            .is_some_and(|waiter| waiter.group.as_deref().is_none_or(|group| group == wake.group));
        if !fits {
            return None;
        }
        let mut notice = self.notice(name, &wake.group)?;
        notice.again = wake.again;
        let waiter = self.waiters.remove(name).expect("looked up above");
        Some(AnsweredWait { ticket: waiter.ticket, name: name.to_string(), notice })
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
            Some(group) => self.groups[&self.locate(group)?].members.iter().cloned().collect(),
            None => self.participants.keys().cloned().collect(),
        };
        Ok(names
            .into_iter()
            .map(|name| {
                let participant = self.participants.get(&name);
                // Its own daemon says, when the host asks it (MIP-4, section 7).
                let liveness = if split_machine(&name).is_some() {
                    Liveness::Unreachable
                } else if name == HUMAN {
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

    // ------------------------------------------------------------------------------------------
    // Groups and their policy (MIP-4, section 8)

    pub fn groups(&self) -> Vec<GroupSummary> {
        self.groups
            .iter()
            .map(|(name, group)| GroupSummary {
                name: name.clone(),
                members: group.members.iter().cloned().collect(),
                policy: group.policy.clone(),
            })
            .collect()
    }

    /// A group's policy, while this machine keeps or holds a copy of the group.
    pub fn policy(&self, group: &str) -> Option<&Policy> {
        self.groups.get(group).map(|group| &group.policy)
    }

    /// Makes a group, with `policy` or the default, and the caller its first member.
    pub fn group_new(
        &mut self,
        caller: &Caller,
        group: &str,
        policy: Option<Policy>,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Changed, Refusal> {
        check_group(group)?;
        if let Some(policy) = &policy {
            policy.check()?;
        }
        if self.groups.contains_key(group) {
            return Err(Refusal::GroupExists { group: group.to_string() });
        }
        let by = self.identify(caller, presence)?;
        self.ensure_group(group, &by, now_ms)?;
        self.add_member(group, &by, now_ms)?;
        let mut seq = None;
        if let Some(policy) = policy.filter(|policy| *policy != Policy::default()) {
            seq = Some(self.set_policy(group, &by, policy, now_ms)?);
        }
        self.save()?;
        Ok(Changed::by(&by, group, seq))
    }

    /// Replaces a group's policy but for whether it is paused, which only `pause` and `resume`
    /// change: they forget and make the wakes a pause holds, and a file that leaves `paused` out
    /// would otherwise unpause the group with nobody woken for what it held.
    pub fn group_set(
        &mut self,
        caller: &Caller,
        group: &str,
        policy: Policy,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Changed, Refusal> {
        policy.check()?;
        self.kept(group)?;
        let by = self.acting(caller, presence)?;
        self.permitted(group, &by, Action::SetPolicy)?;
        let after = self.groups[group].head();
        let policy = Policy { paused: self.groups[group].policy.paused, ..policy };
        let seq = self.set_policy(group, &by, policy, now_ms)?;
        self.save()?;
        let mut changed = Changed::by(&by, group, Some(seq));
        changed.tell = self.tell(group, after, None);
        Ok(changed)
    }

    /// Adds and removes members. Names are read as a post's `--to` reads them, so a pane can
    /// be added before its agent ever joined anything, and `found` holds what other machines
    /// said a name means there, as [`Self::post_found`] takes it.
    #[allow(clippy::too_many_arguments)]
    pub fn group_members(
        &mut self,
        caller: &Caller,
        group: &str,
        add: &[String],
        remove: &[String],
        found: &[String],
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Changed, Refusal> {
        self.kept(group)?;
        let by = self.acting(caller, presence)?;
        if !add.is_empty() {
            self.permitted(group, &by, Action::Add)?;
        }
        if !remove.is_empty() {
            self.permitted(group, &by, Action::Remove)?;
        }
        let mut changed = Changed::by(&by, group, None);
        let after = self.groups[group].head();
        // Told from before the change too, so the machine of a member removed there lets it go.
        let before = self.tell(group, after, None);
        for name in add {
            check_addressee(name)?;
            let name = self.addressee(name, &by, found, Some(group), presence)?;
            if !self.groups[group].members.contains(&name) {
                self.add_member(group, &name, now_ms)?;
                changed.added.push(name);
            }
        }
        for name in remove {
            let Some(name) = self.member_named(group, name)?.or_else(|| self.lookup_name(name))
            else {
                return Err(Refusal::NoSuchParticipant { name: name.clone() });
            };
            if !self.groups[group].members.contains(&name) {
                return Err(Refusal::NotAMember { name, group: group.to_string() });
            }
            self.remove_member(group, &name, now_ms)?;
            let kept_to_it = self
                .waiters
                .get(&name)
                .is_some_and(|waiter| waiter.group.as_deref() == Some(group));
            if kept_to_it && let Some(waiter) = self.waiters.remove(&name) {
                changed.ended.push(waiter.ticket);
            }
            changed.removed.push(name);
        }
        let any = !changed.added.is_empty() || !changed.removed.is_empty();
        changed.seq = any.then(|| self.groups[group].head());
        changed.tell = self.tell(group, after, None);
        for told in before {
            if !changed.tell.iter().any(|telling| telling.machine == told.machine) {
                changed.tell.push(told);
            }
        }
        self.save()?;
        Ok(changed)
    }

    /// Pauses a group: its posts are kept and wake nobody but the human until it is resumed.
    /// What its members were woken for and have not read is forgotten, so a ring still waiting
    /// for its pane is not typed while the group is paused, and resuming wakes them afresh.
    pub fn pause(
        &mut self,
        caller: &Caller,
        group: &str,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Changed, Refusal> {
        self.kept(group)?;
        let by = self.acting(caller, presence)?;
        self.permitted(group, &by, Action::Pause)?;
        if self.groups[group].policy.paused {
            return Ok(Changed::by(&by, group, None));
        }
        let seq = self.append(
            group,
            What::Changed { by: by.clone(), change: Change::Paused, policy: None },
            now_ms,
        )?;
        self.groups.get_mut(group).expect("looked up above").policy.paused = true;
        self.forget_wakes(group);
        self.save()?;
        let mut changed = Changed::by(&by, group, Some(seq));
        changed.tell = self.tell(group, seq - 1, None);
        Ok(changed)
    }

    /// Resumes a paused group, and wakes each member once for what it has unread there - the
    /// same wakes a post would have, so the host delivers them the same way.
    pub fn resume(
        &mut self,
        caller: &Caller,
        group: &str,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Posted, Refusal> {
        self.kept(group)?;
        let by = self.acting(caller, presence)?;
        self.permitted(group, &by, Action::Resume)?;
        let mut posted = Posted {
            author: by.clone(),
            group: group.to_string(),
            seq: self.groups[group].head(),
            reached: Vec::new(),
            wakes: Vec::new(),
            answered: Vec::new(),
            tell: Vec::new(),
            unsaved: None,
        };
        if !self.groups[group].policy.paused {
            return Ok(posted);
        }
        posted.seq = self.append(
            group,
            What::Changed { by: by.clone(), change: Change::Resumed, policy: None },
            now_ms,
        )?;
        self.groups.get_mut(group).expect("looked up above").policy.paused = false;
        posted.tell = self.tell(group, posted.seq - 1, None);
        self.wake_resumed(group, &by, &mut posted, presence, now_ms);
        if let Err(Refusal::Store { error }) = self.save() {
            posted.unsaved = Some(error);
        }
        Ok(posted)
    }

    /// Deletes a group kept here, with its log, letting every member go (MIP-4, section 8).
    /// Nothing unread holds it back: deleting is deliberate, and `log` reads a group first.
    /// The log goes before the saved state does, so a crash between the two leaves a policy
    /// with no log, which a restore drops, rather than a log whose policy went back to the
    /// default. For the same reason a save that fails once the log is gone still answers the
    /// group deleted, so its waits end and the other machines are told.
    pub fn group_delete(
        &mut self,
        caller: &Caller,
        group: &str,
        presence: &dyn Presence,
    ) -> Result<Deleted, Refusal> {
        self.kept(group)?;
        let by = self.acting(caller, presence)?;
        self.permitted(group, &by, Action::Delete)?;
        let record = &self.groups[group];
        let entries = record.log.len() as u64;
        let let_go: Vec<String> = record.members.iter().cloned().collect();
        let forget = self.tell(group, 0, None).into_iter().map(|tell| tell.machine).collect();
        self.store.remove(group).map_err(|error| Refusal::Store { error })?;
        let ended = self.drop_group(group);
        let mut deleted =
            Deleted { by, group: group.to_string(), entries, let_go, ended, forget, unsaved: None };
        if let Err(Refusal::Store { error }) = self.save() {
            deleted.unsaved = Some(error);
        }
        Ok(deleted)
    }

    /// Forgets a group and everything this machine's participants held of it: where each had
    /// read to, what each was woken for, and any wait kept to it, whose tickets it returns.
    fn drop_group(&mut self, group: &str) -> Vec<u64> {
        self.groups.remove(group);
        self.unanswered.remove(group);
        self.lately_reached.remove(group);
        for participant in self.participants.values_mut() {
            participant.cursors.remove(group);
            participant.woken.remove(group);
            participant.rewoken.remove(group);
        }
        let waiting: Vec<String> = self
            .waiters
            .iter()
            .filter(|(_, waiter)| waiter.group.as_deref() == Some(group))
            .map(|(name, _)| name.clone())
            .collect();
        waiting
            .into_iter()
            .filter_map(|name| self.waiters.remove(&name))
            .map(|w| w.ticket)
            .collect()
    }

    /// Forgets the replica of `group`, named as `peer` names it, which its home deleted or no
    /// longer keeps. None when nothing here held it.
    pub fn forget_replica(&mut self, peer: &Peer, group: &str) -> Option<Forgot> {
        let key = peer.inward(group);
        if self.groups.get(&key).is_none_or(|kept| kept.home.as_deref() != Some(&peer.name)) {
            return None;
        }
        let human = self.groups[&key].members.contains(HUMAN);
        let ended = self.drop_group(&key);
        let wakes = if human {
            vec![Wake { name: HUMAN.to_string(), via: Via::Human, notice: self.human_notice(&key) }]
        } else {
            Vec::new()
        };
        let unsaved = match self.save() {
            Err(Refusal::Store { error }) => Some(error),
            _ => None,
        };
        Some(Forgot { group: key, ended, wakes, unsaved })
    }

    fn set_policy(
        &mut self,
        group: &str,
        by: &str,
        policy: Policy,
        now_ms: u64,
    ) -> Result<u64, Refusal> {
        let change = What::Changed {
            by: by.to_string(),
            change: Change::SetPolicy,
            policy: Some(Box::new(policy.clone())),
        };
        let seq = self.append(group, change, now_ms)?;
        self.groups.get_mut(group).expect("a group that exists").policy = policy;
        Ok(seq)
    }

    /// Forgets what this machine's members of a paused group were woken for and have not read.
    pub(crate) fn forget_wakes(&mut self, group: &str) {
        let members: Vec<String> = self.groups[group].members.iter().cloned().collect();
        for name in members {
            if let Some(participant) = self.participants.get_mut(&name) {
                participant.woken.remove(group);
                participant.rewoken.remove(group);
            }
        }
    }

    /// Wakes each of this machine's members of a resumed group, but the one who resumed it,
    /// once for what it has unread there.
    pub(crate) fn wake_resumed(
        &mut self,
        group: &str,
        by: &str,
        posted: &mut Posted,
        presence: &dyn Presence,
        now_ms: u64,
    ) {
        let members: Vec<String> = self.groups[group].members.iter().cloned().collect();
        for name in members {
            let here = split_machine(&name).is_none();
            if here && name != by && self.notice(&name, group).is_some() {
                let reach = self.reach(&name, group, posted, presence, now_ms);
                posted.reached.push((name, reach));
            }
        }
    }

    /// A group kept here, which its policy and membership are changed on: a replica's are its
    /// home's to change.
    /// Found by the name every other verb takes, so a bare name that means a replica is
    /// refused as kept elsewhere rather than as no such group.
    fn kept(&self, group: &str) -> Result<&Group, Refusal> {
        let key = self.locate(group)?;
        let kept = &self.groups[&key];
        match &kept.home {
            None => Ok(kept),
            Some(machine) => Err(Refusal::KeptElsewhere { group: key, machine: machine.clone() }),
        }
    }

    /// The member of `group` that `name` means: that member, or else the one on another machine
    /// going by it, which is not a participant here (MIP-4, section 11).
    fn member_named(&self, group: &str, name: &str) -> Result<Option<String>, Refusal> {
        let name = self.own(name);
        let members = &self.groups[group].members;
        if members.contains(name) {
            return Ok(Some(name.to_string()));
        }
        let elsewhere: Vec<String> = members
            .iter()
            .filter(|member| split_machine(member).is_some_and(|(base, _)| base == name))
            .cloned()
            .collect();
        match elsewhere.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.clone())),
            _ => Err(Refusal::WhichParticipant { name: name.to_string(), candidates: elsewhere }),
        }
    }

    /// Refuses `name` an action its group's `membership` does not give it.
    pub(crate) fn permitted(&self, group: &str, name: &str, action: Action) -> Result<(), Refusal> {
        let policy = &self.group(group)?.policy;
        if policy.permits(name) {
            return Ok(());
        }
        Err(Refusal::NotPermitted {
            name: name.to_string(),
            group: group.to_string(),
            action,
            permitted: policy.membership.clone(),
        })
    }

    /// The participant a name means, by its own name or its pane's, without making one.
    fn lookup_name(&self, name: &str) -> Option<String> {
        if self.participants.contains_key(name) {
            return Some(name.to_string());
        }
        self.by_pane(name)
    }

    /// Who a post's `--to` names, each once, and never its author.
    pub(crate) fn addressees(
        &mut self,
        author: &str,
        to: &[String],
        found: &[String],
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Vec<String>, Refusal> {
        let mut addressees: Vec<String> = Vec::new();
        for name in to {
            check_addressee(name)?;
            let name = self.addressee(name, author, found, group, presence)?;
            if name == author {
                return Err(Refusal::AddressedSelf);
            }
            if !addressees.contains(&name) {
                addressees.push(name);
            }
        }
        Ok(addressees)
    }

    /// The participant `name` means in a post's `--to`: a member of the group named, or of the
    /// author's groups, by exactly that name, or the one on another machine by that name, so
    /// `critic` reaches `critic@devenv` (MIP-4, section 11); else a participant by that name;
    /// else the one in the pane of that name; else, for a pane with an agent in it, a
    /// participant made for it, named after the pane, which is what lets a post reach an agent
    /// that never joined (MIP-4, section 3). The human is made on first address too. Else the
    /// one another machine said it has, in `found`.
    fn addressee(
        &mut self,
        name: &str,
        author: &str,
        found: &[String],
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<String, Refusal> {
        let name = self.own(name);
        if name == HUMAN
            && let Some(home) = self.human_elsewhere(presence)
        {
            return Ok(home.human());
        }
        let scope: Vec<String> = match group {
            Some(group) => vec![group.to_string()],
            None => self.memberships(author),
        };
        let members: BTreeSet<&String> =
            scope.iter().flat_map(|group| &self.groups[group].members).collect();
        if members.contains(&name.to_string()) {
            return Ok(name.to_string());
        }
        let elsewhere: Vec<String> = members
            .iter()
            .filter(|member| split_machine(member).is_some_and(|(base, _)| base == name))
            .map(|member| (*member).clone())
            .collect();
        match elsewhere.as_slice() {
            [] => {}
            [only] => return Ok(only.clone()),
            _ => {
                return Err(Refusal::WhichParticipant {
                    name: name.to_string(),
                    candidates: elsewhere,
                });
            }
        }
        if self.participants.contains_key(name) {
            return Ok(name.to_string());
        }
        if let Some(holder) = self.by_pane(name) {
            return Ok(holder);
        }
        if self.make_addressed(name, presence) {
            return Ok(name.to_string());
        }
        let found: Vec<String> = found
            .iter()
            .filter(|each| {
                *each == name || split_machine(each).is_some_and(|(base, _)| base == name)
            })
            .cloned()
            .collect();
        match found.as_slice() {
            [] => Err(Refusal::NoSuchParticipant { name: name.to_string() }),
            [only] => Ok(only.clone()),
            _ => Err(Refusal::WhichParticipant { name: name.to_string(), candidates: found }),
        }
    }

    /// Makes the participant a name means when it is not one yet: the human, where it is homed,
    /// or an agent that may start in the pane of that name. Says whether it did.
    pub(crate) fn make_addressed(&mut self, name: &str, presence: &dyn Presence) -> bool {
        let human = name == HUMAN && self.human_elsewhere(presence).is_none();
        if !human && !presence.has_pane(name) {
            return false;
        }
        let mut participant = Participant::named(name);
        if name != HUMAN {
            participant.pane = Some(name.to_string());
        }
        self.participants.insert(name.to_string(), participant);
        true
    }

    // ------------------------------------------------------------------------------------------
    // Who is asking

    /// The participant the caller already is, without making one.
    pub(crate) fn lookup(&self, caller: &Caller) -> Option<String> {
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

    pub(crate) fn by_pane(&self, pane: &str) -> Option<String> {
        self.participants
            .values()
            .find(|participant| participant.pane.as_deref() == Some(pane))
            .map(|participant| participant.name.clone())
    }

    /// The caller as far as its addresses identify it: a pane counts only while detection has
    /// found an agent in it, since a person's own shell in a pane is the human (MIP-4, section 3).
    pub(crate) fn addressed(caller: &Caller, presence: &dyn Presence) -> Caller {
        let mut caller = caller.clone();
        caller.pane = caller.pane.filter(|pane| presence.agent_in(pane));
        caller
    }

    /// Refuses to make the caller `name` while `name` is a live participant in another
    /// session: `join --name` and `--as` alike (MIP-4, section 3). The caller's addresses would
    /// replace the live one's, and it would never be woken again. A caller carrying no address,
    /// such as a script, moves nothing, so it may act as anyone.
    pub(crate) fn may_become(
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
    pub(crate) fn identify(
        &mut self,
        caller: &Caller,
        presence: &dyn Presence,
    ) -> Result<String, Refusal> {
        let (name, _) = self.identify_reporting(caller, presence)?;
        self.seen(&name, caller);
        Ok(name)
    }

    /// Records that `peer`'s daemon dialed this one. The app's daemon is the one that dials, so
    /// the human is homed there, and this daemon has none of its own (MIP-4, section 10).
    pub fn dialed_by(&mut self, peer: &Peer) -> Result<(), Refusal> {
        self.human_home =
            Some(HumanHome { machine: peer.name.clone(), calls_us: peer.calls_us.clone() });
        self.save()
    }

    /// Where the human is homed when that is not here: the machine that last dialed this
    /// daemon, unless a window attends it, since then the app runs on this machine.
    pub(crate) fn human_elsewhere(&self, presence: &dyn Presence) -> Option<&HumanHome> {
        self.human_home.as_ref().filter(|_| !presence.attended())
    }

    /// Refuses to act as this machine's human where it has none.
    pub(crate) fn human_here(&self, presence: &dyn Presence) -> Result<(), Refusal> {
        self.human_elsewhere(presence).map_or(Ok(()), |home| Err(home.refusal()))
    }

    /// Who acts in a verb that needs no cursor of its own: a post, or a change to a group's
    /// policy or members. [`Self::identify`], but where the human is homed elsewhere a person's
    /// shell is that human, as a member from there is named, and is made no participant here:
    /// its cursors are kept at its home.
    pub(crate) fn acting(
        &mut self,
        caller: &Caller,
        presence: &dyn Presence,
    ) -> Result<String, Refusal> {
        let addressed = Self::addressed(caller, presence);
        let named = addressed.as_name.clone().or_else(|| self.lookup(&addressed));
        match self.human_elsewhere(presence) {
            Some(home) if named.as_deref() == Some(HUMAN) => Ok(home.human()),
            _ => self.identify(caller, presence),
        }
    }

    /// Notes that `name` ran a verb, which is how its hooks are seen to run.
    fn seen(&mut self, name: &str, caller: &Caller) {
        if name != HUMAN {
            self.seen_ms.insert(name.to_string(), caller.at_ms);
        }
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
            if name == HUMAN {
                self.human_here(presence)?;
            }
            self.may_become(name, caller, presence)?;
            self.adopt(name, caller);
            return Ok((name.clone(), false));
        }
        if let Some(name) = self.lookup(caller) {
            if name == HUMAN {
                self.human_here(presence)?;
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
    pub(crate) fn adopt(&mut self, name: &str, caller: &Caller) {
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
    pub(crate) fn made_by_address(&self, caller: &Caller) -> Option<String> {
        let pane = caller.pane.as_ref()?;
        let made = self.participants.get(pane)?;
        (made.pane.as_ref() == Some(pane) && made.inbox.is_none()).then(|| pane.clone())
    }

    /// Folds `from` into `into`: its groups, where it had read to, and what it was woken for.
    /// An agent addressed by its pane that then joins under a name of its own is one agent, and
    /// the brief it was rung for must be readable under that name.
    pub(crate) fn absorb(&mut self, into: &str, from: &str) {
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

    /// The group a name means: a group kept here by that name, or a replica by that exact
    /// name, or else the one replica of a group by that name on another machine, so `review`
    /// finds `review@devenv` when nothing here is called that (MIP-4, section 11).
    pub(crate) fn locate(&self, name: &str) -> Result<String, Refusal> {
        let name = self.own(name);
        if self.groups.contains_key(name) {
            return Ok(name.to_string());
        }
        let replicas: Vec<String> = self
            .groups
            .keys()
            .filter(|key| split_machine(key).is_some_and(|(base, _)| base == name))
            .cloned()
            .collect();
        match replicas.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(Refusal::NoSuchGroup { group: name.to_string() }),
            _ => Err(Refusal::WhichGroup { candidates: replicas }),
        }
    }

    /// Refuses a change to a group this machine only holds a replica of: that goes to its home.
    fn here(&self, group: &str) -> Result<(), Refusal> {
        match &self.groups[group].home {
            None => Ok(()),
            Some(machine) => {
                Err(Refusal::Unreachable { group: group.to_string(), machine: machine.clone() })
            }
        }
    }

    pub(crate) fn memberships(&self, name: &str) -> Vec<String> {
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
                let group = self.locate(group)?;
                if !self.groups[&group].members.contains(name) {
                    return Err(Refusal::NotAMember { name: name.to_string(), group });
                }
                Ok(vec![group])
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
        let group = Group {
            policy: Policy::default(),
            members: BTreeSet::new(),
            log: Vec::new(),
            home: None,
        };
        self.groups.insert(name.to_string(), group);
        if let Err(refusal) = self.append(name, What::Created { by: by.to_string() }, now_ms) {
            self.groups.remove(name);
            return Err(refusal);
        }
        Ok(true)
    }

    /// Adds a member, whose reading starts from now: history from before it joined is what
    /// `log` is for, and counting it as unread would refuse the newcomer's first post.
    pub(crate) fn add_member(
        &mut self,
        group: &str,
        name: &str,
        now_ms: u64,
    ) -> Result<(), Refusal> {
        if self.groups[group].members.contains(name) {
            return Ok(());
        }
        let seq = self.append(group, What::Joined { who: name.to_string() }, now_ms)?;
        if let Some(participant) = self.participants.get_mut(name) {
            participant.cursors.insert(group.to_string(), seq);
            participant.woken.remove(group);
            participant.rewoken.remove(group);
        }
        Ok(())
    }

    pub(crate) fn remove_member(
        &mut self,
        group: &str,
        name: &str,
        now_ms: u64,
    ) -> Result<(), Refusal> {
        self.append(group, What::Left { who: name.to_string() }, now_ms)?;
        if let Some(participant) = self.participants.get_mut(name) {
            participant.cursors.remove(group);
            participant.woken.remove(group);
            participant.rewoken.remove(group);
        }
        Ok(())
    }

    /// Which group a post goes to (MIP-4, section 2). `group` is one [`Self::locate`] found.
    pub(crate) fn resolve_group(
        &mut self,
        author: &str,
        addressees: &[String],
        group: Option<&str>,
        now_ms: u64,
    ) -> Result<String, Refusal> {
        if let Some(group) = group {
            self.check_members(group, author, addressees)?;
            return Ok(group.to_string());
        }
        match self.shared_group(author, addressees)? {
            Some(group) => Ok(group),
            None if addressees.is_empty() => Err(Refusal::NoGroup),
            None => {
                // The human's groups are made where its cursors are kept.
                if let Some(home) = &self.human_home
                    && author == home.human()
                {
                    return Err(home.refusal());
                }
                let mut everyone: Vec<&str> = vec![author];
                everyone.extend(addressees.iter().map(String::as_str));
                let group = self.pair_name(&everyone)?;
                self.ensure_group(&group, author, now_ms)?;
                for name in everyone {
                    self.add_member(&group, name, now_ms)?;
                }
                Ok(group)
            }
        }
    }

    /// The name of the group of exactly `everyone`: [`pair_group`]'s, unless a group here
    /// already goes by it with somebody else in it - the pair of another machine's `critic` and
    /// this one's `builder` is named as this machine's own pair is - and then the first of
    /// `-2`, `-3`, ... that is free. A group by the name holding only some of them is theirs: one
    /// of them left it.
    fn pair_name(&self, everyone: &[&str]) -> Result<String, Refusal> {
        let base = pair_group(everyone);
        if check_group(&base).is_err() {
            return Err(Refusal::PairTooLong { group: base });
        }
        let theirs = |name: &str| match self.groups.get(name) {
            Some(group) => group.members.iter().all(|member| everyone.contains(&member.as_str())),
            None => !self.groups.keys().any(|other| other.eq_ignore_ascii_case(name)),
        };
        let mut candidate = base.clone();
        for suffix in 2.. {
            if theirs(&candidate) {
                break;
            }
            candidate = format!("{base}-{suffix}");
        }
        if check_group(&candidate).is_err() {
            return Err(Refusal::PairTooLong { group: candidate });
        }
        Ok(candidate)
    }

    /// Refuses a post to `group` unless its author and every addressee are members.
    pub(crate) fn check_members(
        &self,
        group: &str,
        author: &str,
        addressees: &[String],
    ) -> Result<(), Refusal> {
        let members = &self.group(group)?.members;
        if !members.contains(author) {
            return Err(Refusal::NotAMember { name: author.to_string(), group: group.to_string() });
        }
        if let Some(outside) = addressees.iter().find(|name| !members.contains(*name)) {
            return Err(Refusal::AddresseeNotInGroup {
                name: outside.clone(),
                group: group.to_string(),
            });
        }
        Ok(())
    }

    /// The one group of the author's that every addressee is in, or with no addressees the
    /// author's only group; nothing when there is none (MIP-4, section 2).
    pub(crate) fn shared_group(
        &self,
        author: &str,
        addressees: &[String],
    ) -> Result<Option<String>, Refusal> {
        let shared: Vec<String> = self
            .memberships(author)
            .into_iter()
            .filter(|group| {
                let members = &self.groups[group].members;
                addressees.iter().all(|name| members.contains(name))
            })
            .collect();
        match shared.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.clone())),
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

    pub(crate) fn cursor(&self, name: &str, group: &str) -> u64 {
        self.participants
            .get(name)
            .and_then(|participant| participant.cursors.get(group))
            .copied()
            .unwrap_or(0)
    }

    /// Messages by others after the cursor: what the guard counts. Notices never count, or
    /// every join would make every pending post stale, as it did in council v1.
    pub(crate) fn unread_from_others(&self, name: &str, group: &str) -> u64 {
        self.unread_after(name, group, self.cursor(name, group))
    }

    fn unread_after(&self, name: &str, group: &str, cursor: u64) -> u64 {
        let count = self.groups[group]
            .log
            .iter()
            .filter(|entry| entry.seq > cursor)
            .filter(|entry| entry.author().is_some_and(|author| author != name))
            .count();
        count as u64
    }

    /// Whom a message by `author` wakes: its addressees, or the ring set `policy` gives for its
    /// author (MIP-4, section 5).
    pub(crate) fn targets(
        &self,
        group: &str,
        policy: &Policy,
        author: &str,
        addressees: Vec<String>,
    ) -> Vec<String> {
        if !addressees.is_empty() {
            return addressees;
        }
        let members = &self.groups[group].members;
        members.iter().filter(|member| policy.rings(author, member)).cloned().collect()
    }

    /// The machines other than `except` with a member in `group`, which a new entry there must
    /// reach. None for a replica: only a group's home tells anyone.
    pub(crate) fn tell(&self, group: &str, after: u64, except: Option<&str>) -> Vec<Tell> {
        let group_record = &self.groups[group];
        if group_record.home.is_some() {
            return Vec::new();
        }
        let machines: BTreeSet<&str> = group_record
            .members
            .iter()
            .filter_map(|member| split_machine(member).map(|(_, machine)| machine))
            .filter(|machine| Some(*machine) != except)
            .collect();
        machines
            .into_iter()
            .map(|machine| Tell {
                machine: machine.to_string(),
                group: group.to_string(),
                after,
                targets: Vec::new(),
            })
            .collect()
    }

    /// What to tell `name` about `group`: its unread messages that would wake it, or nothing
    /// when there are none.
    pub(crate) fn notice(&self, name: &str, group: &str) -> Option<Notice> {
        self.notice_under(name, group, &self.groups.get(group)?.policy)
    }

    /// [`Self::notice`], read under `policy` rather than the group's own.
    fn notice_under(&self, name: &str, group: &str, policy: &Policy) -> Option<Notice> {
        let kept = self.groups.get(group)?;
        let cursor = self.cursor(name, group);
        // A paused group wakes nobody but the human, however it would (MIP-4, section 8).
        if policy.paused && name != HUMAN {
            return None;
        }
        let mut notice: Option<Notice> = None;
        for entry in kept.log.iter().filter(|entry| entry.seq > cursor) {
            let What::Message { author, to, urgent, .. } = &entry.what else { continue };
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
                urgent: 0,
                from: Vec::new(),
                again: false,
            });
            notice.last = entry.seq;
            notice.count += 1;
            notice.to_you += u64::from(addressed);
            notice.urgent += u64::from(*urgent);
            if !notice.from.contains(author) {
                notice.from.push(author.clone());
            }
        }
        notice
    }

    /// Reaches a member a new message is for: as [`Self::reach`] does, unless the group is
    /// paused, which wakes nobody but the human (MIP-4, section 8).
    pub(crate) fn reach_member(
        &mut self,
        name: &str,
        group: &str,
        urgent: bool,
        posted: &mut Posted,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Reach {
        if self.groups[group].policy.paused && name != HUMAN {
            return Reach::Paused;
        }
        let policy = self.groups[group].policy.clone();
        self.reach_under(name, group, &policy, urgent, posted, presence, now_ms)
    }

    /// Wakes `name` for `group` if nothing has since the last time it read.
    pub(crate) fn reach(
        &mut self,
        name: &str,
        group: &str,
        posted: &mut Posted,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Reach {
        let policy = self.groups[group].policy.clone();
        self.reach_under(name, group, &policy, false, posted, presence, now_ms)
    }

    /// [`Self::reach`], for messages posted under `policy` rather than the group's policy now.
    /// An `urgent` post wakes even a participant woken already and not since read: once per
    /// batch is the rule for what can wait until a turn ends, and an urgent post cannot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reach_under(
        &mut self,
        name: &str,
        group: &str,
        policy: &Policy,
        urgent: bool,
        posted: &mut Posted,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Reach {
        let Some(participant) = self.participants.get(name) else {
            return Reach::Gone;
        };
        if participant.gone {
            return Reach::Gone;
        }
        let Some(notice) = self.notice_under(name, group, policy) else {
            return Reach::Waiting;
        };
        let woken_before = participant.woken.contains(group);
        let waiting = self.waiters.get(name).is_some_and(|waiter| {
            waiter.group.as_deref().is_none_or(|filter| filter == group)
                && !(waiter.due && woken_before && !urgent)
        });
        let hooked = self.hooked(name, now_ms, presence);
        if name == HUMAN {
            posted.wakes.push(Wake {
                name: name.to_string(),
                via: Via::Human,
                notice: notice.clone(),
            });
        }
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
        // Every message that wakes the human is told to the windows, above, rather than once
        // per batch: a person reading the transcript moves no cursor, so a batch would never
        // end (MIP-4, section 10). It is told with no window attending too, so that the next
        // window to attend finds it.
        if name == HUMAN {
            return if presence.attended() { Reach::Woken } else { Reach::Waiting };
        }
        let via = Self::via(participant, presence);
        // Woken already only while something is there to take this too: an agent whose pane
        // closed, or who left it, was woken for nothing it will read (MIP-4, section 7).
        let to_come = participant
            .pane
            .as_ref()
            .is_some_and(|pane| presence.doorbell(pane) == Ringable::AgentToCome);
        if participant.woken.contains(group)
            && !urgent
            && via.is_ok()
            && (to_come || presence.alive(participant))
        {
            return Reach::AlreadyWoken;
        }
        // Mid-turn, its next PostToolUse hook reads this; at the turn's end its Stop hook is told.
        if hooked {
            if woken_before {
                return Reach::AlreadyWoken;
            }
            participant.woken.insert(group.to_string());
            return Reach::Woken;
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

    /// What waits for this machine's human in `group` now, a count of 0 when nothing does: for
    /// a change other than a message or a read that can move it, since a policy decides which
    /// messages ring the human (MIP-4, sections 8 and 10).
    pub fn human_notice(&self, group: &str) -> Notice {
        let member = self.groups.get(group).is_some_and(|kept| kept.members.contains(HUMAN));
        member.then(|| self.notice(HUMAN, group)).flatten().unwrap_or_else(|| Notice {
            group: group.to_string(),
            first: 0,
            last: 0,
            count: 0,
            to_you: 0,
            urgent: 0,
            from: Vec::new(),
            again: false,
        })
    }

    /// What waits for the human in each group it is in, for a host that is starting to tell
    /// the windows (MIP-4, section 10).
    pub fn human_notices(&self) -> Vec<Notice> {
        self.groups
            .iter()
            .filter(|(_, group)| group.members.contains(HUMAN))
            .filter_map(|(name, _)| self.notice(HUMAN, name))
            .collect()
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
        now_ms: u64,
    ) -> (Vec<Wake>, Option<String>) {
        // Its Stop hook's wait, connected, asks for the "still unread" wake itself (`wait --due`).
        if self.hooked(name, now_ms, presence) {
            return (Vec::new(), None);
        }
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

    pub(crate) fn save(&mut self) -> Result<(), Refusal> {
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
                .filter(|(_, group)| group.home.is_none())
                .map(|(name, group)| GroupRecord {
                    name: name.clone(),
                    policy: group.policy.clone(),
                })
                .collect(),
            human_home: self.human_home.clone(),
        }
    }
}

pub(crate) fn check_body(body: &str) -> Result<(), Refusal> {
    if body.trim().is_empty() {
        return Err(Refusal::EmptyBody);
    }
    if body.len() > LARGEST_BODY {
        return Err(Refusal::BodyTooLarge { bytes: body.len() });
    }
    Ok(())
}

/// Whether an address a participant holds is being replaced by another, rather than given for
/// the first time or given again.
fn replaced<T: PartialEq>(held: Option<&T>, given: Option<&T>) -> bool {
    held.is_some() && given.is_some() && held != given
}
