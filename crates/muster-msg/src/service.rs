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
        }
    }
}

/// Whether a participant is still there, as the host can tell: for a Claude session, whether
/// its inbox socket still accepts a connection.
pub trait Presence {
    fn alive(&self, participant: &Participant) -> bool;
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
    pub groups: Vec<String>,
    pub inbox: Option<String>,
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
}

/// A wake for the host to deliver, outside whatever lock it holds this under, and to report
/// back with [`Messaging::delivered`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    pub name: String,
    pub inbox: Inbox,
    pub notice: Notice,
}

/// What a post did for each participant it was meant to wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Woken,
    /// Woken for this group already, and has not read since: it will read this too.
    AlreadyWoken,
    /// Nothing can wake it, so it sees the message when it next reads.
    Waiting,
    Gone,
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
                check_participant(name)?;
                self.may_become(name, caller, presence)?;
                let took_over = self.participants.get(name).is_some_and(|existing| {
                    existing.inbox.is_some() && existing.inbox != caller.inbox
                });
                self.adopt(name, caller);
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
        now_ms: u64,
    ) -> Result<Left, Refusal> {
        let name = self
            .lookup(caller)
            .filter(|name| self.participants.contains_key(name))
            .ok_or_else(|| Refusal::NotAParticipant {
                name: Some(caller.as_name.clone().unwrap_or_else(|| HUMAN.to_string())),
            })?;
        let left = if let Some(group) = group {
            let members = &self.group(group)?.members;
            if !members.contains(&name) {
                return Err(Refusal::NotAMember { name, group: group.to_string() });
            }
            self.remove_member(group, &name, now_ms)?;
            Left { name, groups: vec![group.to_string()], stopped: false, ended: None }
        } else {
            let groups = self.memberships(&name);
            for group in &groups {
                self.remove_member(group, &name, now_ms)?;
            }
            self.participants.remove(&name);
            self.waiters.remove(&name);
            Left { name, groups, stopped: true, ended: None }
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
            if *name == author {
                return Err(Refusal::AddressedSelf);
            }
            if !self.participants.contains_key(name) {
                if name != HUMAN {
                    return Err(Refusal::NoSuchParticipant { name: name.clone() });
                }
                self.participants.insert(HUMAN.to_string(), Participant::named(HUMAN));
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
            let reach = self.reach(&target, &group, &mut posted);
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
        if participant.inbox.as_ref() != Some(&wake.inbox) {
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
                    liveness,
                    name,
                }
            })
            .collect())
    }

    // ------------------------------------------------------------------------------------------
    // Who is asking

    /// The participant the caller already is, without making one.
    fn lookup(&self, caller: &Caller) -> Option<String> {
        if let Some(name) = &caller.as_name {
            return Some(name.clone());
        }
        if let Some(inbox) = &caller.inbox {
            return self.by_inbox(inbox);
        }
        Some(HUMAN.to_string())
    }

    fn by_inbox(&self, inbox: &Inbox) -> Option<String> {
        self.participants
            .values()
            .find(|participant| participant.inbox.as_ref() == Some(inbox))
            .map(|participant| participant.name.clone())
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
        let elsewhere = caller.inbox.is_some() && existing.inbox != caller.inbox;
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
        if let Some(name) = &caller.as_name {
            check_participant(name)?;
            self.may_become(name, caller, presence)?;
            self.adopt(name, caller);
            return Ok((name.clone(), false));
        }
        if let Some(inbox) = &caller.inbox {
            if let Some(name) = self.by_inbox(inbox) {
                self.adopt(&name, caller);
                return Ok((name, false));
            }
            let base = default_name(caller.directory.as_deref());
            let name = self.free_name(&base, presence);
            let took_over = self.participants.contains_key(&name);
            self.adopt(&name, caller);
            return Ok((name, took_over));
        }
        self.participants.entry(HUMAN.to_string()).or_insert_with(|| Participant::named(HUMAN));
        Ok((HUMAN.to_string(), false))
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
    /// inbox belongs to one participant, so any other holding it lets go, and one that is left
    /// holding nothing - a default name the caller has since replaced with its own - goes.
    fn adopt(&mut self, name: &str, caller: &Caller) {
        let participant =
            self.participants.entry(name.to_string()).or_insert_with(|| Participant::named(name));
        if caller.inbox.is_some() {
            // A session that takes over a name has been woken for nothing yet, whatever the
            // session before it was told.
            if participant.inbox != caller.inbox {
                participant.woken.clear();
            }
            participant.inbox.clone_from(&caller.inbox);
            participant.gone = false;
        }
        if caller.pane.is_some() {
            participant.pane.clone_from(&caller.pane);
        }
        let Some(inbox) = &caller.inbox else { return };
        let others: Vec<String> = self
            .participants
            .values()
            .filter(|other| other.name != name && other.inbox.as_ref() == Some(inbox))
            .map(|other| other.name.clone())
            .collect();
        for other in others {
            if self.memberships(&other).is_empty() {
                self.participants.remove(&other);
            } else if let Some(other) = self.participants.get_mut(&other) {
                other.inbox = None;
            }
        }
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
    fn reach(&mut self, name: &str, group: &str, posted: &mut Posted) -> Reach {
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
        if participant.woken.contains(group) {
            return Reach::AlreadyWoken;
        }
        let Some(inbox) = participant.inbox.clone() else {
            return Reach::Waiting;
        };
        participant.woken.insert(group.to_string());
        posted.wakes.push(Wake { name: name.to_string(), inbox, notice });
        Reach::Woken
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
