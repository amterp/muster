//! Groups that span machines (MIP-4, section 11). A group is kept on one daemon, its home, which
//! numbers and stores its log; a daemon with a member of a group kept elsewhere holds a replica
//! of it under `name@machine`, forwards its members' changes to the home, and wakes only its own
//! members. How the calls travel is the host's: this is what each side does with them.
//!
//! Each machine writes its own members bare and another machine's as `name@machine`, in its own
//! name for that machine. So whatever crosses the link is written as the sender writes it, and
//! the receiver turns it into its own names on arrival ([`Peer::inward`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::names::{
    check_addressee, check_group, check_participant, is_human, is_machine, split_machine,
};
use crate::service::{Draft, Group, check_body};
use crate::{
    Action, AnsweredWait, Caller, Change, Entry, HUMAN, Joined, Left, Liveness, Member, Messaging,
    Policy, Posted, Presence, Reach, Refusal, Store, Via, Wake, What,
};

/// Another machine's daemon, as a link to it knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// What this machine calls it.
    pub name: String,
    /// What it calls this machine.
    pub calls_us: String,
}

/// The machine the human is homed on, as a daemon there that dialed this one introduced it
/// (MIP-4, section 10): the app runs there, and its daemon is the one that dials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanHome {
    /// What this machine calls it.
    pub machine: String,
    /// What it calls this machine, which is how a group kept here is named there.
    pub calls_us: String,
}

impl HumanHome {
    /// The human, as this machine writes a member on that one.
    pub fn human(&self) -> String {
        format!("{HUMAN}@{}", self.machine)
    }

    pub(crate) fn refusal(&self) -> Refusal {
        Refusal::HumanElsewhere { machine: self.machine.clone(), calls_us: self.calls_us.clone() }
    }
}

impl Peer {
    /// A name as the peer wrote it, as this machine writes it: the peer's own members gain its
    /// name, and this machine's lose theirs.
    pub fn inward(&self, name: &str) -> String {
        match split_machine(name) {
            Some((base, machine)) if machine == self.calls_us => base.to_string(),
            Some(_) => name.to_string(),
            None => format!("{name}@{}", self.name),
        }
    }

    fn entry(&self, entry: Entry) -> Entry {
        let what = match entry.what {
            What::Message { author, to, body, urgent } => What::Message {
                author: self.inward(&author),
                to: to.iter().map(|name| self.inward(name)).collect(),
                body,
                urgent,
            },
            What::Created { by } => What::Created { by: self.inward(&by) },
            What::Joined { who } => What::Joined { who: self.inward(&who) },
            What::Left { who } => What::Left { who: self.inward(&who) },
            What::Changed { by, change } => What::Changed { by: self.inward(&by), change },
        };
        Entry { what, ..entry }
    }

    /// A policy's names turned like any other, but for `*` and `@human`, which name roles
    /// rather than participants: `@human` is the human wherever it is homed.
    pub fn policy(&self, policy: Policy) -> Policy {
        let name =
            |name: String| if name == "*" || name == HUMAN { name } else { self.inward(&name) };
        let names = |names: Vec<String>| names.into_iter().map(name).collect::<Vec<_>>();
        let table = |table: BTreeMap<String, Vec<String>>| {
            table.into_iter().map(|(author, set)| (name(author), names(set))).collect()
        };
        Policy {
            ring: table(policy.ring),
            allow: table(policy.allow),
            membership: names(policy.membership),
            urgent: names(policy.urgent),
            paused: policy.paused,
        }
    }

    fn refusal(&self, refusal: Refusal) -> Refusal {
        let group = |group: String| self.inward(&group);
        let name = |name: String| self.inward(&name);
        let role =
            |name: String| if name == "*" || name == HUMAN { name } else { self.inward(&name) };
        match refusal {
            Refusal::NoSuchGroup { group: g } => Refusal::NoSuchGroup { group: group(g) },
            Refusal::NotAMember { name: n, group: g } => {
                Refusal::NotAMember { name: name(n), group: group(g) }
            }
            Refusal::AddresseeNotInGroup { name: n, group: g } => {
                Refusal::AddresseeNotInGroup { name: name(n), group: group(g) }
            }
            Refusal::Unread { group: g, count } => Refusal::Unread { group: group(g), count },
            Refusal::NoSuchParticipant { name: n } => Refusal::NoSuchParticipant { name: name(n) },
            Refusal::WhichParticipant { name: n, candidates } => Refusal::WhichParticipant {
                name: n,
                candidates: candidates.into_iter().map(name).collect(),
            },
            Refusal::NotAllowed { addressee, group: g, allowed } => Refusal::NotAllowed {
                addressee: name(addressee),
                group: group(g),
                allowed: allowed.into_iter().map(role).collect(),
            },
            Refusal::NotUrgent { group: g, urgent } => Refusal::NotUrgent {
                group: group(g),
                urgent: urgent.into_iter().map(role).collect(),
            },
            Refusal::NotPermitted { name: n, group: g, action, permitted } => {
                Refusal::NotPermitted {
                    name: name(n),
                    group: group(g),
                    action,
                    permitted: permitted.into_iter().map(role).collect(),
                }
            }
            other => other,
        }
    }
}

/// A group's new entries, which the host sends `machine`: those after `after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tell {
    pub machine: String,
    pub group: String,
    pub after: u64,
    /// The members there the new message is for, which are unreachable if it cannot be sent.
    pub targets: Vec<String>,
}

/// What one machine asks of another. Groups are named as their home names them, and every
/// other name as the asker writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// Whether a group by this name is kept there.
    Find {
        group: String,
    },
    Join {
        name: String,
        group: String,
        head: u64,
    },
    Leave {
        name: String,
        group: String,
        head: u64,
    },
    /// A post, with how far its author had read, which the home's guard compares against.
    Post {
        author: String,
        group: String,
        to: Vec<String>,
        body: String,
        urgent: bool,
        cursor: u64,
        head: u64,
    },
    Since {
        group: String,
        after: u64,
    },
    /// The asked machine's own members of the group, and what each is doing.
    Who {
        group: String,
    },
    /// The participant a name means there: see [`Messaging::whom`].
    Whom {
        name: String,
    },
}

impl Call {
    /// Refuses a call whose names are not ones an honest asker sends, before any is turned into
    /// this machine's: the group is one kept here, so bare, and whoever joins, leaves or posts is
    /// the asker's own participant, so bare too. `builder@<us>` there would become this
    /// machine's `builder` on arrival.
    pub fn check(&self) -> Result<(), Refusal> {
        if let Call::Whom { name } = self {
            return check_participant(name);
        }
        check_group(self.group())?;
        match self {
            Call::Join { name, .. } | Call::Leave { name, .. } => check_participant(name),
            Call::Post { author, to, .. } => {
                check_participant(author)?;
                to.iter().try_for_each(|name| check_addressee(name))
            }
            Call::Find { .. } | Call::Since { .. } | Call::Who { .. } | Call::Whom { .. } => Ok(()),
        }
    }

    /// The group the call is about, as its home names it; nothing for [`Call::Whom`].
    pub fn group(&self) -> &str {
        match self {
            Call::Find { group }
            | Call::Join { group, .. }
            | Call::Leave { group, .. }
            | Call::Post { group, .. }
            | Call::Since { group, .. }
            | Call::Who { group } => group,
            Call::Whom { .. } => "",
        }
    }
}

/// A call for the host to send to the machine a group is kept on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Away {
    pub machine: String,
    pub call: Call,
}

/// Where a request is to be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Here,
    Away(Away),
    /// No group by this name is here, and linked machines may keep one: the host asks each
    /// with [`Call::Find`], and joins `group@machine` if one does, or creates it here if none.
    Ask {
        group: String,
    },
}

/// A group's entries after some point, with the policy they are read under, as the machine
/// keeping the group writes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caught {
    pub group: String,
    pub policy: Policy,
    pub entries: Vec<Entry>,
    /// The home holds entries after these, left for the next page: see [`CAUGHT_BYTES`].
    #[serde(default)]
    pub more: bool,
}

/// How many bytes of message bodies one [`Caught`] carries before it leaves the rest for the
/// next page. Half of what a link carries in a frame, since a body may be a megabyte and the
/// entries around the bodies take room too; one entry always goes, whatever its size.
pub const CAUGHT_BYTES: usize = 8 << 20;

impl Caught {
    /// Refuses entries whose names are not ones an honest home sends: its group bare, since a
    /// group written `review@<us>` would land on one kept here, and every name one a participant
    /// could have, this machine's own included - the human posts from a shell there too.
    pub fn check(&self) -> Result<(), Refusal> {
        check_group(&self.group)?;
        self.policy.check()?;
        for entry in &self.entries {
            match &entry.what {
                What::Message { author, to, .. } => {
                    check_addressee(author)?;
                    to.iter().try_for_each(|name| check_addressee(name))?;
                }
                What::Created { by } | What::Changed { by, .. } => check_addressee(by)?,
                What::Joined { who } | What::Left { who } => check_addressee(who)?,
            }
        }
        Ok(())
    }
}

/// The answer to a [`Call`]. Each that changed a group carries the entries after the asker's
/// head, so a replica that fell behind catches up in the same reply, a refusal included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Found(bool),
    Joined {
        seq: u64,
        caught: Caught,
    },
    Left {
        caught: Caught,
    },
    Posted {
        seq: u64,
        reached: Vec<(String, Reach)>,
        caught: Caught,
    },
    Caught(Caught),
    Members(Vec<Member>),
    /// The participant a [`Call::Whom`] asked about, if there is one.
    Named(Option<String>),
    Refused {
        refusal: Refusal,
        caught: Option<Caught>,
    },
}

impl Reply {
    /// Refuses a reply whose names are not ones an honest home sends, as [`Caught::check`] does.
    pub fn check(&self) -> Result<(), Refusal> {
        match self {
            Reply::Found(_) => Ok(()),
            Reply::Joined { caught, .. } | Reply::Left { caught } | Reply::Caught(caught) => {
                caught.check()
            }
            Reply::Posted { reached, caught, .. } => {
                reached.iter().try_for_each(|(name, _)| check_addressee(name))?;
                caught.check()
            }
            Reply::Members(members) => {
                members.iter().try_for_each(|member| check_addressee(&member.name))
            }
            Reply::Named(name) => name.as_deref().map_or(Ok(()), check_participant),
            Reply::Refused { caught, .. } => caught.as_ref().map_or(Ok(()), Caught::check),
        }
    }
}

/// A call answered, and what the answering host still has to do: wake its own members, end the
/// waits the call answered, and send the new entries on to other machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub reply: Reply,
    pub wakes: Vec<Wake>,
    pub answered: Vec<AnsweredWait>,
    pub tell: Vec<Tell>,
    pub unsaved: Option<String>,
}

/// What a replica's new entries did on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub reached: Vec<(String, Reach)>,
    pub wakes: Vec<Wake>,
    pub answered: Vec<AnsweredWait>,
    /// Waits kept to the group by members here its home removed, which the host ends as a
    /// leave does.
    pub ended: Vec<u64>,
    pub unsaved: Option<String>,
    /// The replica's head, when its home said it holds more: where to fetch the next page from.
    pub more: Option<u64>,
}

/// Whom a replica's last batch of new entries reached, and which entries those were.
///
/// A link coming up refetches every replica, and a refetch that runs late takes a post before its
/// home sends it on: the home's send then finds every entry held already and would answer that it
/// reached nobody. Remembered so that answer says whom the entries reached when they were taken.
#[derive(Debug, Clone)]
pub(crate) struct Lately {
    from: u64,
    to: u64,
    reached: Vec<(String, Reach)>,
}

/// Whom the entries of a batch held already reached when they were taken, as far as the replica's
/// last batch says.
fn reached_before(lately: Option<&Lately>, head: u64, entries: &[Entry]) -> Vec<(String, Reach)> {
    let taken = entries.iter().map(|entry| entry.seq).filter(|seq| *seq <= head);
    match lately {
        Some(lately) if taken.into_iter().any(|seq| (lately.from..=lately.to).contains(&seq)) => {
            lately.reached.clone()
        }
        _ => Vec::new(),
    }
}

/// A request answered by another machine, as it ends on this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Found(bool),
    Joined(Joined),
    Left(Left),
    /// Who was reached, on both machines. The wakes on this one are in [`Settle::applied`].
    Posted(Posted),
    Members(Vec<Member>),
    /// Who a name means there, as this machine writes it.
    Named(Option<String>),
    Caught,
    /// The entries left a gap after this head, which the host fills with [`Call::Since`].
    Gap(u64),
}

/// How a request another machine answered ends here, and what the entries its answer carried
/// did on this machine, which the host delivers whatever the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settle {
    pub result: Result<Settled, Refusal>,
    pub applied: Applied,
}

/// Where a group named in a join is.
enum Place {
    Here,
    Elsewhere { key: String, machine: String },
    Nowhere,
}

impl<S: Store> Messaging<S> {
    /// A link to `peer` is up.
    pub fn linked(&mut self, peer: &Peer) {
        self.linked.insert(peer.name.clone());
        self.met.insert(peer.name.clone());
        self.called.insert(peer.calls_us.clone());
    }

    /// `name` as this machine writes it, when it is written as another machine writes one of
    /// this machine's own - `review@devenv` on the devenv is `review` - so a name copied from
    /// another machine's answer means here what it meant there.
    pub(crate) fn own<'a>(&self, name: &'a str) -> &'a str {
        let ours = |machine: &str| {
            self.called.contains(machine)
                || self.human_home.as_ref().is_some_and(|home| home.calls_us == machine)
        };
        match split_machine(name) {
            Some((base, machine)) if ours(machine) => base,
            _ => name,
        }
    }

    /// Refuses making a group here by a name nothing here holds while a machine this one has
    /// linked to cannot be asked whether it keeps one; `group new` makes one here regardless.
    pub fn unchecked(&self, group: &str) -> Result<(), Refusal> {
        let machines: Vec<String> = self.met.difference(&self.linked).cloned().collect();
        if machines.is_empty() {
            return Ok(());
        }
        Err(Refusal::Unchecked { group: group.to_string(), machines })
    }

    /// The link to `machine` is down: changes to its groups are refused until it is back.
    pub fn unlinked(&mut self, machine: &str) {
        self.linked.remove(machine);
    }

    /// A call that changes `group`, a replica, went unanswered: it may have been made at the home
    /// all the same, so the replica may lack the entry until the home is next heard from.
    pub fn unanswered(&mut self, group: &str) {
        self.unanswered.insert(group.to_string());
    }

    /// The machine a replica may be behind: there is no link to it now, or a change sent there
    /// went unanswered.
    pub fn behind(&self, group: &str) -> Option<&str> {
        let key = self.locate(group).ok()?;
        let home = self.groups.get(&key)?.home.as_deref()?;
        (!self.linked.contains(home) || self.unanswered.contains(&key)).then_some(home)
    }

    /// Groups kept on `machine` that this one replicates, with each replica's head: what to
    /// refetch when a link to it comes up. A cursor alone counts, with a head of nothing, since
    /// replicas are not kept and a restart leaves only the cursors (MIP-4, section 12).
    pub fn replicas_of(&self, machine: &str) -> Vec<(String, u64)> {
        let mut found: BTreeMap<String, u64> = BTreeMap::new();
        let keys = self.participants.values().flat_map(|participant| participant.cursors.keys());
        for key in keys {
            if let Some((base, at)) = split_machine(key)
                && at == machine
            {
                found.entry(base.to_string()).or_insert(0);
            }
        }
        for (key, group) in &self.groups {
            if group.home.as_deref() == Some(machine)
                && let Some((base, _)) = split_machine(key)
            {
                found.insert(base.to_string(), group.head());
            }
        }
        found.into_iter().collect()
    }

    // ------------------------------------------------------------------------------------------
    // On the asker's machine

    /// Where `join` is answered. A group kept elsewhere is joined through its home, so the
    /// caller becomes a participant here first.
    pub fn route_join(
        &mut self,
        caller: &Caller,
        name: Option<&str>,
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Route, Refusal> {
        // The human's cursors are kept at its home whatever the group, so a person's shell
        // where that is elsewhere hears so before anything asks where the group is.
        let joining =
            name.map(str::to_string).or_else(|| self.lookup(&Self::addressed(caller, presence)));
        if joining.as_deref() == Some(HUMAN) {
            self.human_here(presence)?;
        }
        let Some(group) = group else { return Ok(Route::Here) };
        let (key, machine) = match self.place(group)? {
            Place::Here => return Ok(Route::Here),
            Place::Nowhere if self.linked.is_empty() => {
                self.unchecked(group)?;
                return Ok(Route::Here);
            }
            Place::Nowhere => return Ok(Route::Ask { group: group.to_string() }),
            Place::Elsewhere { key, machine } => (key, machine),
        };
        self.reachable(&key, &machine)?;
        let name = match name {
            Some(name) => {
                self.become_named(caller, name, presence)?;
                name.to_string()
            }
            None => self.identify(caller, presence)?,
        };
        self.save()?;
        let call = Call::Join { name, group: base(&key), head: self.head_of(&key) };
        Ok(Route::Away(Away { machine, call }))
    }

    /// Where `post` is answered. A post to a group kept elsewhere is checked here against the
    /// replica, then forwarded with its author's cursor, which the home's guard compares with
    /// the log as it appends (MIP-4, section 4).
    ///
    /// A `--to` name that means nobody here is refused as [`Refusal::NoSuchParticipant`]; the
    /// host asks linked machines with [`Call::Whom`] and routes again with what they said in
    /// `found`, as [`Self::post_found`] takes it.
    pub fn route_post(
        &mut self,
        caller: &Caller,
        draft: &Draft<'_>,
        presence: &dyn Presence,
    ) -> Result<Route, Refusal> {
        check_body(draft.body)?;
        let author = self.acting(caller, presence)?;
        let group = draft.group.map(|group| self.locate(group)).transpose()?;
        let addressees =
            self.addressees(&author, draft.to, draft.found, group.as_deref(), presence)?;
        let key = match group {
            Some(key) => key,
            None => match self.shared_group(&author, &addressees)? {
                Some(key) => key,
                None => return Ok(Route::Here),
            },
        };
        let Some(machine) = self.groups[&key].home.clone() else { return Ok(Route::Here) };
        // The human's posts elsewhere go from its home, where its cursors are: this machine
        // forwards only its own members'.
        if let Some(home) = self.human_elsewhere(presence)
            && author == home.human()
        {
            return Err(home.refusal());
        }
        self.reachable(&key, &machine)?;
        self.check_members(&key, &author, &addressees)?;
        let unread = if is_human(&author) { 0 } else { self.unread_from_others(&author, &key) };
        if unread > 0 {
            return Err(Refusal::Unread { group: key, count: unread });
        }
        self.save()?;
        let call = Call::Post {
            cursor: self.cursor(&author, &key),
            head: self.head_of(&key),
            group: base(&key),
            author,
            to: addressees,
            body: draft.body.to_string(),
            urgent: draft.urgent,
        };
        Ok(Route::Away(Away { machine, call }))
    }

    /// What `leave` must ask of other machines before it is answered here: each group kept
    /// elsewhere that it leaves.
    pub fn route_leave(
        &mut self,
        caller: &Caller,
        group: Option<&str>,
        presence: &dyn Presence,
    ) -> Result<Vec<Away>, Refusal> {
        let caller = &Self::addressed(caller, presence);
        if let Some(home) = self.human_elsewhere(presence)
            && self.lookup(caller).as_deref() == Some(HUMAN)
        {
            return Err(home.refusal());
        }
        let Some(name) = self.lookup(caller).filter(|name| self.participants.contains_key(name))
        else {
            return Ok(Vec::new());
        };
        let keys: Vec<String> = match group {
            Some(group) => vec![self.locate(group)?],
            None => self.memberships(&name),
        };
        // A leave from every group is refused whole (MIP-4, section 8), so each group's policy
        // is asked before any group elsewhere is left. A replica holds its home's policy.
        if group.is_none() {
            for key in &keys {
                self.permitted(key, &name, Action::Leave)?;
            }
        }
        let mut away = Vec::new();
        for key in keys {
            let Some(machine) = self.groups[&key].home.clone() else { continue };
            self.reachable(&key, &machine)?;
            if !self.groups[&key].members.contains(&name) {
                return Err(Refusal::NotAMember { name, group: key });
            }
            let call =
                Call::Leave { name: name.clone(), group: base(&key), head: self.head_of(&key) };
            away.push(Away { machine, call });
        }
        Ok(away)
    }

    /// Whom to ask about `group`'s members on other machines: its home, for a replica, and for
    /// a group kept here each machine with a member in it. A machine with no link now is not
    /// asked, and its members stay [`Liveness::Unreachable`].
    pub fn route_who(&self, group: Option<&str>) -> Result<Vec<Away>, Refusal> {
        let Some(group) = group else { return Ok(Vec::new()) };
        let key = self.locate(group)?;
        let kept = &self.groups[&key];
        let machines: BTreeSet<String> = match &kept.home {
            Some(home) => BTreeSet::from([home.clone()]),
            None => kept
                .members
                .iter()
                .filter_map(|member| split_machine(member).map(|(_, machine)| machine.to_string()))
                .collect(),
        };
        let call = Call::Who { group: base(&key) };
        Ok(machines
            .into_iter()
            .filter(|machine| self.linked.contains(machine))
            .map(|machine| Away { machine, call: call.clone() })
            .collect())
    }

    /// Ends a request another machine answered: catches the replica up with what the answer
    /// carried, reaches this machine's members for it, and turns the answer into this machine's
    /// names.
    pub fn settle(
        &mut self,
        peer: &Peer,
        call: &Call,
        reply: Reply,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Settle {
        let mut applied = Applied::default();
        let result = self.settled(peer, call, reply, presence, now_ms, &mut applied);
        Settle { result, applied }
    }

    fn settled(
        &mut self,
        peer: &Peer,
        call: &Call,
        reply: Reply,
        presence: &dyn Presence,
        now_ms: u64,
        applied: &mut Applied,
    ) -> Result<Settled, Refusal> {
        let key = peer.inward(call.group());
        // A gap here is left for the next refetch: these answers follow the asker's own head.
        let mut apply = |service: &mut Self, caught| {
            service.apply_into(peer, caught, presence, now_ms, applied).unwrap_or_default()
        };
        match reply {
            Reply::Found(found) => Ok(Settled::Found(found)),
            Reply::Named(name) => Ok(Settled::Named(name.map(|name| peer.inward(&name)))),
            Reply::Members(members) => Ok(Settled::Members(
                members
                    .into_iter()
                    .map(|member| Member { name: peer.inward(&member.name), ..member })
                    .collect(),
            )),
            Reply::Caught(caught) => match self.apply_into(peer, caught, presence, now_ms, applied)
            {
                Ok(_) => Ok(Settled::Caught),
                Err(head) => Ok(Settled::Gap(head)),
            },
            Reply::Refused { refusal, caught } => {
                if let Some(caught) = caught {
                    apply(self, caught);
                }
                Err(peer.refusal(refusal))
            }
            Reply::Joined { seq, caught } => {
                let Call::Join { name, .. } = call else { return Err(mismatched(call)) };
                // The cursor first, so nothing from before the join counts as unread to it.
                if let Some(participant) = self.participants.get_mut(name) {
                    participant.cursors.entry(key.clone()).or_insert(seq);
                    participant.woken.remove(&key);
                }
                apply(self, caught);
                self.save()?;
                Ok(Settled::Joined(Joined {
                    name: name.clone(),
                    group: Some(key),
                    created: false,
                    took_over: false,
                    tell: Vec::new(),
                }))
            }
            Reply::Left { caught } => {
                let Call::Leave { name, .. } = call else { return Err(mismatched(call)) };
                apply(self, caught);
                if let Some(participant) = self.participants.get_mut(name) {
                    participant.cursors.remove(&key);
                    participant.woken.remove(&key);
                    participant.rewoken.remove(&key);
                }
                let kept_to_it = self
                    .waiters
                    .get(name)
                    .is_some_and(|waiter| waiter.group.as_deref() == Some(key.as_str()));
                let ended = kept_to_it.then(|| self.waiters.remove(name)).flatten();
                self.save()?;
                Ok(Settled::Left(Left {
                    name: name.clone(),
                    groups: vec![key],
                    stopped: false,
                    ended: ended.map(|waiter| waiter.ticket),
                    tell: Vec::new(),
                }))
            }
            Reply::Posted { seq, reached, caught } => {
                let Call::Post { author, .. } = call else { return Err(mismatched(call)) };
                let mut all: Vec<(String, Reach)> =
                    reached.into_iter().map(|(name, reach)| (peer.inward(&name), reach)).collect();
                all.extend(apply(self, caught));
                Ok(Settled::Posted(Posted {
                    author: author.clone(),
                    group: key,
                    seq,
                    reached: all,
                    wakes: Vec::new(),
                    answered: Vec::new(),
                    tell: Vec::new(),
                    unsaved: None,
                }))
            }
        }
    }

    /// [`Self::apply`], adding what it did to `applied`, and returning whom it reached.
    fn apply_into(
        &mut self,
        peer: &Peer,
        caught: Caught,
        presence: &dyn Presence,
        now_ms: u64,
        applied: &mut Applied,
    ) -> Result<Vec<(String, Reach)>, u64> {
        let done = self.apply(peer, caught, presence, now_ms)?;
        applied.reached.extend(done.reached.iter().cloned());
        applied.wakes.extend(done.wakes);
        applied.answered.extend(done.answered);
        applied.ended.extend(done.ended);
        applied.unsaved = applied.unsaved.take().or(done.unsaved);
        applied.more = applied.more.or(done.more);
        Ok(done.reached)
    }

    /// Takes a group's entries from its home into the replica, and reaches this machine's own
    /// members for each new message, as a post here would, under the policy the home sent.
    /// A pause or a resume there does here what it does to the home's own members. Entries it
    /// holds already are skipped, so the same entries may arrive twice; entries that would
    /// leave a gap are not taken, and the head they would follow is returned so the host can
    /// fetch what is missing.
    pub fn apply(
        &mut self,
        peer: &Peer,
        caught: Caught,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Result<Applied, u64> {
        let key = peer.inward(&caught.group);
        let more = caught.more;
        let ring_before = self.groups.get(&key).map(|group| group.policy.ring.clone());
        let group = self.groups.entry(key.clone()).or_insert_with(|| Group::replica(&peer.name));
        let head = group.head();
        let earlier = reached_before(self.lately_reached.get(&key), head, &caught.entries);
        let fresh: Vec<Entry> =
            caught.entries.into_iter().filter(|entry| entry.seq > head).collect();
        if fresh.first().is_some_and(|first| first.seq != head + 1) {
            return Err(head);
        }
        // A batch carries the policy at its end, and the log records that a policy was set but
        // not which. So a message rings under the replica's previous policy until the batch sets
        // a new one, and under the final policy after; a replica that held nothing has no
        // previous policy. Pauses are exact, since each pause and resume is an entry.
        let policy = peer.policy(caught.policy);
        let mut ringing = if head > 0 { group.policy.clone() } else { policy.clone() };
        let mut paused = paused_before(&fresh, policy.paused);
        group.policy = policy;
        let mut posted = Posted {
            author: String::new(),
            group: key.clone(),
            seq: head,
            reached: earlier,
            wakes: Vec::new(),
            answered: Vec::new(),
            tell: Vec::new(),
            unsaved: None,
        };
        let carried = posted.reached.len();
        let mut resumed = None;
        // Reached once the batch is in, and only those still members then: a replica refetched
        // from nothing replays its whole log, and a member who left since was woken for it
        // already, before it left. Each is woken under the policy of its last message posted
        // while the group was not paused, and held if every one came while it was.
        let mut targets: Vec<Target> = Vec::new();
        let mut human_left = false;
        let mut forgotten = false;
        for entry in fresh {
            let entry = peer.entry(entry);
            let group = self.groups.get_mut(&key).expect("made above");
            match &entry.what {
                What::Joined { who } => {
                    group.members.insert(who.clone());
                    self.cursor_from_join(who, &key, entry.seq, presence);
                }
                What::Left { who } => {
                    group.members.remove(who);
                    human_left |= who == HUMAN;
                }
                What::Changed { change: Change::Paused, .. } => {
                    paused = true;
                    resumed = None;
                    forgotten = true;
                    self.forget_wakes(&key);
                }
                What::Changed { by, change: Change::Resumed } => {
                    paused = false;
                    resumed = Some(by.clone());
                }
                What::Changed { change: Change::SetPolicy, .. } => {
                    ringing = group.policy.clone();
                }
                _ => {}
            }
            let group = self.groups.get_mut(&key).expect("made above");
            let message = match &entry.what {
                What::Message { author, to, urgent, .. } => {
                    Some((author.clone(), to.clone(), *urgent))
                }
                _ => None,
            };
            group.log.push(entry);
            let Some((author, to, urgent)) = message else { continue };
            let live = (!paused).then(|| Policy { paused: false, ..ringing.clone() });
            for target in self.targets(&key, &ringing, &author, to) {
                Target::add(&mut targets, target, live.as_ref(), urgent);
            }
        }
        self.reach_batch(&key, targets, forgotten, &mut posted, presence, now_ms);
        self.remember_reached(&key, head, &posted.reached[carried..]);
        if let Some(by) = resumed
            && !self.groups[&key].policy.paused
        {
            self.wake_resumed(&key, &by, &mut posted, presence, now_ms);
        }
        self.tell_the_human_if_changed(&key, human_left, ring_before.as_ref(), &mut posted);
        keep_last_human_wake(&mut posted.wakes);
        self.unanswered.remove(&key);
        let more = more.then(|| self.groups[&key].head());
        // Only once the batch reaches the home's head: until then a member may yet rejoin in a
        // later page, and one joining now may not have reached its Joined entry.
        let ended = if more.is_none() { self.let_go_of(&key) } else { Vec::new() };
        let unsaved = match self.save() {
            Err(Refusal::Store { error }) => Some(error),
            _ => None,
        };
        Ok(Applied {
            reached: posted.reached,
            wakes: posted.wakes,
            answered: posted.answered,
            ended,
            unsaved,
            more,
        })
    }

    /// What the windows show for the human may be stale after a batch: nothing waits once it
    /// has left, and a new ring set there may ring it for what is already unread, or no longer.
    fn tell_the_human_if_changed(
        &self,
        key: &str,
        human_left: bool,
        ring_before: Option<&BTreeMap<String, Vec<String>>>,
        posted: &mut Posted,
    ) {
        let human_here = self.groups[key].members.contains(HUMAN);
        let rings_otherwise = ring_before.is_some_and(|ring| *ring != self.groups[key].policy.ring);
        if (human_left && !human_here) || (rings_otherwise && human_here) {
            let notice = self.human_notice(key);
            posted.wakes.push(Wake { name: HUMAN.to_string(), via: Via::Human, notice });
        }
    }

    /// Remembers whom the entries a batch added after `head` reached, if it added any.
    fn remember_reached(&mut self, key: &str, head: u64, reached: &[(String, Reach)]) {
        let to = self.groups[key].head();
        if to > head {
            let lately = Lately { from: head + 1, to, reached: reached.to_vec() };
            self.lately_reached.insert(key.to_string(), lately);
        }
    }

    /// Reaches this machine's members a replica's new messages are for, each under the policy
    /// its message was posted under, or as held by a pause if every one of them came while the
    /// group was paused (see [`Self::apply`]). `forgotten` says the batch paused the group.
    fn reach_batch(
        &mut self,
        key: &str,
        targets: Vec<Target>,
        forgotten: bool,
        posted: &mut Posted,
        presence: &dyn Presence,
        now_ms: u64,
    ) {
        for Target { name, under, urgent } in targets {
            let member = self.groups[key].members.contains(&name);
            if !member || !self.participants.contains_key(&name) {
                continue;
            }
            let reach = match under {
                Some(policy) => {
                    self.reach_under(&name, key, &policy, urgent, posted, presence, now_ms)
                }
                None if name == HUMAN => self.reach(&name, key, posted, presence, now_ms),
                None => Reach::Paused,
            };
            posted.reached.push((name, reach));
        }
        // Woken for a message from before a pause the batch also holds, as the home woke its own
        // members when it was posted; the pause then forgot that, as it did at the home.
        if forgotten && self.groups[key].policy.paused {
            self.forget_wakes(key);
        }
    }

    // ------------------------------------------------------------------------------------------
    // The person, carried to the human's home

    /// Where the human is homed, when the caller is this machine's person and that is elsewhere:
    /// a request the service refuses them as `human_elsewhere`, or as `kept_elsewhere` for a
    /// group kept there, is carried there and done as the human (MIP-4, section 10).
    pub fn person_elsewhere(&self, caller: &Caller, presence: &dyn Presence) -> Option<HumanHome> {
        let home = self.human_elsewhere(presence)?;
        let addressed = Self::addressed(caller, presence);
        let named = addressed.as_name.clone().or_else(|| self.lookup(&addressed));
        (named.as_deref() == Some(HUMAN)).then(|| home.clone())
    }

    /// The machine `group` is kept on, when that is another one.
    pub fn home_of(&self, group: &str) -> Option<String> {
        let key = self.locate(group).ok()?;
        self.groups.get(&key)?.home.clone()
    }

    /// A group named in a request carried to the human's home, as this machine writes it, for
    /// the home to turn into its own: one this machine knows by the name as it knows it, and one
    /// it does not as the home's, which the home reads as its own bare name.
    pub fn carrying_group(&self, name: &str) -> String {
        let name = self.own(name);
        if let Ok(key) = self.locate(name) {
            return key;
        }
        self.at_home(name)
    }

    /// A participant named in a request carried to the human's home, as this machine writes it:
    /// one this machine knows - its own, an agent in a pane here, or a member of a group it
    /// holds, by its own name - as it knows it, the human as the home's, and anyone else as the
    /// home's too, which the home reads as its own bare name and asks around for if it has none.
    pub fn carrying_name(&self, name: &str, presence: &dyn Presence) -> String {
        let name = self.own(name);
        if name == HUMAN {
            return self.at_home(name);
        }
        let here = self.participants.contains_key(name)
            || self.by_pane(name).is_some()
            || presence.has_pane(name)
            || split_machine(name).is_some();
        if here {
            return name.to_string();
        }
        let members: BTreeSet<&String> = self
            .groups
            .values()
            .flat_map(|group| &group.members)
            .filter(|member| split_machine(member).is_some_and(|(base, _)| base == name))
            .collect();
        match members.into_iter().collect::<Vec<_>>().as_slice() {
            [only] => (*only).clone(),
            _ => self.at_home(name),
        }
    }

    /// A policy in a request carried to the human's home, its names as [`Self::carrying_name`]
    /// writes them but for `*` and `@human`, which name roles rather than participants.
    pub fn carrying_policy(&self, policy: Policy, presence: &dyn Presence) -> Policy {
        let name = |name: String| {
            if name == "*" || name == HUMAN { name } else { self.carrying_name(&name, presence) }
        };
        let names = |names: Vec<String>| names.into_iter().map(name).collect::<Vec<_>>();
        let table = |table: BTreeMap<String, Vec<String>>| {
            table.into_iter().map(|(author, set)| (name(author), names(set))).collect()
        };
        Policy {
            ring: table(policy.ring),
            allow: table(policy.allow),
            membership: names(policy.membership),
            urgent: names(policy.urgent),
            paused: policy.paused,
        }
    }

    /// A bare `name` as this machine writes the one on the human's home.
    fn at_home(&self, name: &str) -> String {
        match (&self.human_home, split_machine(name)) {
            (Some(home), None) => format!("{name}@{}", home.machine),
            _ => name.to_string(),
        }
    }

    // ------------------------------------------------------------------------------------------
    // On the machine asked

    /// Answers another machine's call.
    pub fn answer(
        &mut self,
        peer: &Peer,
        call: Call,
        presence: &dyn Presence,
        now_ms: u64,
    ) -> Answered {
        let mut answered = Answered {
            reply: Reply::Found(false),
            wakes: Vec::new(),
            answered: Vec::new(),
            tell: Vec::new(),
            unsaved: None,
        };
        let refused = |refusal| Reply::Refused { refusal, caught: None };
        if let Err(refusal) = call.check() {
            answered.reply = refused(refusal);
            return answered;
        }
        answered.reply = match call {
            Call::Find { group } => Reply::Found(self.kept_here(&group)),
            Call::Since { group, after } => {
                self.since(&group, after).map_or_else(refused, Reply::Caught)
            }
            Call::Who { group } => Reply::Members(self.members_here(peer, &group, presence)),
            Call::Whom { name } => Reply::Named(self.whom(&name, presence)),
            Call::Join { name, group, head } => self
                .join_from(peer, &name, &group, head, now_ms, &mut answered)
                .unwrap_or_else(refused),
            Call::Leave { name, group, head } => self
                .leave_from(peer, &name, &group, head, now_ms, &mut answered)
                .unwrap_or_else(refused),
            Call::Post { author, group, to, body, urgent, cursor, head } => {
                let post = Forwarded { author, group, to, body, urgent, cursor, head };
                self.post_from(peer, post, presence, now_ms, &mut answered)
            }
        };
        answered
    }

    /// The participant `name` means on this machine, for another that has nobody by it: one by
    /// that name, or the one in the pane of that name, or the pane itself while an agent may
    /// start there, as a post's `--to` reads it here. The human is not asked for: it is homed
    /// where the app runs, and every machine names it already.
    pub fn whom(&self, name: &str, presence: &dyn Presence) -> Option<String> {
        if name == HUMAN {
            return None;
        }
        if self.participants.contains_key(name) {
            return Some(name.to_string());
        }
        self.by_pane(name).or_else(|| presence.has_pane(name).then(|| name.to_string()))
    }

    /// Starts the cursor of `name`, if it is one of this machine's participants, at its join to
    /// the replica `key` when the join's own answer did not, so nothing from before the join is
    /// unread to it. A name the home added that is nobody here yet - a pane named in `--to` or
    /// `group add` on another machine, or the human - becomes a participant as it would here.
    fn cursor_from_join(&mut self, name: &str, key: &str, seq: u64, presence: &dyn Presence) {
        if !self.participants.contains_key(name) && split_machine(name).is_none() {
            self.make_addressed(name, presence);
        }
        if let Some(participant) = self.participants.get_mut(name) {
            participant.cursors.entry(key.to_string()).or_insert(seq);
        }
    }

    /// Lets each of this machine's participants that is not a member of the replica `key` go of
    /// it, as leaving would, and drops the replica if none is left in it, so it is not fetched
    /// again. Returns the waits they kept to it, which the host ends. Judged by membership once
    /// the batch is in rather than by each `Left`, so a replica replayed from nothing does not
    /// let go of a member who left and came back.
    fn let_go_of(&mut self, key: &str) -> Vec<u64> {
        let members = &self.groups[key].members;
        let gone: Vec<String> =
            self.participants.keys().filter(|name| !members.contains(*name)).cloned().collect();
        if members.iter().all(|member| split_machine(member).is_some()) {
            self.groups.remove(key);
        }
        gone.iter().filter_map(|name| self.let_go(name, key)).collect()
    }

    /// Lets `name`, one of this machine's participants, go of a replica its home took it out of,
    /// as leaving would: returns the wait it kept to that group, which the host ends.
    fn let_go(&mut self, name: &str, key: &str) -> Option<u64> {
        let participant = self.participants.get_mut(name)?;
        participant.cursors.remove(key);
        participant.woken.remove(key);
        participant.rewoken.remove(key);
        let kept_to_it =
            self.waiters.get(name).is_some_and(|waiter| waiter.group.as_deref() == Some(key));
        kept_to_it.then(|| self.waiters.remove(name)).flatten().map(|waiter| waiter.ticket)
    }

    /// A group's entries after `after`, as this machine keeps them.
    pub fn since(&self, group: &str, after: u64) -> Result<Caught, Refusal> {
        if !self.kept_here(group) {
            return Err(Refusal::NoSuchGroup { group: group.to_string() });
        }
        let kept = &self.groups[group];
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut more = false;
        for entry in kept.log.iter().filter(|entry| entry.seq > after) {
            let size = match &entry.what {
                What::Message { body, .. } => body.len(),
                _ => 0,
            };
            if !entries.is_empty() && bytes + size > CAUGHT_BYTES {
                more = true;
                break;
            }
            bytes += size;
            entries.push(entry.clone());
        }
        Ok(Caught { group: group.to_string(), policy: kept.policy.clone(), entries, more })
    }

    fn join_from(
        &mut self,
        peer: &Peer,
        name: &str,
        group: &str,
        head: u64,
        now_ms: u64,
        answered: &mut Answered,
    ) -> Result<Reply, Refusal> {
        if !self.kept_here(group) {
            return Err(Refusal::NoSuchGroup { group: group.to_string() });
        }
        let who = peer.inward(name);
        if !self.groups[group].members.contains(&who) {
            self.permitted(group, &who, Action::Join)?;
        }
        let after = self.groups[group].head();
        self.add_member(group, &who, now_ms)?;
        let seq = self.groups[group].head();
        answered.tell = self.tell(group, after, Some(&peer.name));
        self.save()?;
        Ok(Reply::Joined { seq, caught: self.since(group, head)? })
    }

    fn leave_from(
        &mut self,
        peer: &Peer,
        name: &str,
        group: &str,
        head: u64,
        now_ms: u64,
        answered: &mut Answered,
    ) -> Result<Reply, Refusal> {
        if !self.kept_here(group) {
            return Err(Refusal::NoSuchGroup { group: group.to_string() });
        }
        let who = peer.inward(name);
        if !self.groups[group].members.contains(&who) {
            return Err(Refusal::NotAMember { name: who, group: group.to_string() });
        }
        self.permitted(group, &who, Action::Leave)?;
        let after = self.groups[group].head();
        self.remove_member(group, &who, now_ms)?;
        answered.tell = self.tell(group, after, Some(&peer.name));
        self.save()?;
        Ok(Reply::Left { caught: self.since(group, head)? })
    }

    /// A post forwarded from another machine, held to the same rules as one made here: its
    /// author and addressees are members, and it is refused while its author has unread
    /// messages, counted from the cursor it brought.
    fn post_from(
        &mut self,
        peer: &Peer,
        post: Forwarded,
        presence: &dyn Presence,
        now_ms: u64,
        answered: &mut Answered,
    ) -> Reply {
        let Forwarded { author, group, to, body, urgent, cursor, head } = post;
        let refused = |refusal| Reply::Refused { refusal, caught: None };
        if !self.kept_here(&group) {
            return refused(Refusal::NoSuchGroup { group });
        }
        if let Err(refusal) = check_body(&body) {
            return refused(refusal);
        }
        let author = peer.inward(&author);
        let addressees: Vec<String> = to.iter().map(|name| peer.inward(name)).collect();
        if let Err(refusal) = self.check_members(&group, &author, &addressees) {
            return refused(refusal);
        }
        let caught = |service: &Self| service.since(&group, head).ok();
        let from = Some(peer.name.as_str());
        let posting = self
            .post_as(&author, &group, addressees, &body, urgent, cursor, from, presence, now_ms);
        match posting {
            Ok(posted) => {
                answered.wakes = posted.wakes;
                answered.answered = posted.answered;
                answered.tell = posted.tell;
                answered.unsaved = posted.unsaved;
                match caught(self) {
                    Some(caught) => {
                        Reply::Posted { seq: posted.seq, reached: posted.reached, caught }
                    }
                    None => refused(Refusal::NoSuchGroup { group: group.clone() }),
                }
            }
            Err(refusal @ Refusal::Unread { .. }) => {
                Reply::Refused { refusal, caught: caught(self) }
            }
            Err(refusal) => refused(refusal),
        }
    }

    /// This machine's own members of `group` - kept here, or replicated from `peer` - with
    /// what each is doing.
    fn members_here(&self, peer: &Peer, group: &str, presence: &dyn Presence) -> Vec<Member> {
        let key = if self.kept_here(group) { group.to_string() } else { peer.inward(group) };
        let Ok(members) = self.who(Some(&key), presence) else { return Vec::new() };
        members.into_iter().filter(|member| split_machine(&member.name).is_none()).collect()
    }

    // ------------------------------------------------------------------------------------------

    fn kept_here(&self, group: &str) -> bool {
        self.groups.get(group).is_some_and(|kept| kept.home.is_none())
    }

    fn head_of(&self, key: &str) -> u64 {
        self.groups.get(key).map_or(0, Group::head)
    }

    fn reachable(&self, group: &str, machine: &str) -> Result<(), Refusal> {
        if self.linked.contains(machine) {
            Ok(())
        } else {
            Err(Refusal::Unreachable { group: group.to_string(), machine: machine.to_string() })
        }
    }

    fn place(&self, group: &str) -> Result<Place, Refusal> {
        let group = self.own(group);
        if let Some(kept) = self.groups.get(group) {
            return Ok(match &kept.home {
                None => Place::Here,
                Some(machine) => {
                    Place::Elsewhere { key: group.to_string(), machine: machine.clone() }
                }
            });
        }
        if let Some((base, machine)) = split_machine(group) {
            check_group(base)?;
            if !is_machine(machine) {
                return Err(Refusal::BadName { name: group.to_string() });
            }
            return Ok(Place::Elsewhere { key: group.to_string(), machine: machine.to_string() });
        }
        check_group(group)?;
        match self.locate(group) {
            Ok(key) => {
                let machine = self.groups[&key].home.clone().expect("a replica, found by its base");
                Ok(Place::Elsewhere { key, machine })
            }
            Err(Refusal::NoSuchGroup { .. }) => Ok(Place::Nowhere),
            Err(refusal) => Err(refusal),
        }
    }
}

/// Adds what another machine said about its own members of `group` to `members`, which
/// holds that machine's as [`Liveness::Unreachable`] until then. A member it did not name
/// has gone from it.
pub fn heard(members: &mut [Member], machine: &str, heard: &[Member]) {
    for member in members {
        if split_machine(&member.name).is_none_or(|(_, at)| at != machine) {
            continue;
        }
        match heard.iter().find(|said| said.name == member.name) {
            Some(said) => {
                member.liveness = said.liveness;
                member.activity = said.activity;
                member.pane.clone_from(&said.pane);
                member.inbox.clone_from(&said.inbox);
            }
            None => member.liveness = Liveness::Gone,
        }
    }
}

struct Forwarded {
    author: String,
    group: String,
    to: Vec<String>,
    body: String,
    urgent: bool,
    cursor: u64,
    head: u64,
}

/// A member a replica's new messages are for: the policy it is reached under, none when every
/// one of them came while the group was paused, and whether any that came unpaused was urgent.
struct Target {
    name: String,
    under: Option<Policy>,
    urgent: bool,
}

impl Target {
    /// Adds a message for `name`, posted under `live`, none while the group was paused: a later
    /// policy stands for an earlier one, and a paused message wakes nobody, urgent or not.
    fn add(targets: &mut Vec<Target>, name: String, live: Option<&Policy>, urgent: bool) {
        let urgent = urgent && live.is_some();
        match targets.iter_mut().find(|known| known.name == name) {
            Some(known) => {
                known.under = live.cloned().or(known.under.take());
                known.urgent |= urgent;
            }
            None => targets.push(Target { name, under: live.cloned(), urgent }),
        }
    }
}

/// A replica's key without its machine: the name its home keeps it under.
fn base(key: &str) -> String {
    split_machine(key).map_or(key, |(base, _)| base).to_string()
}

fn mismatched(call: &Call) -> Refusal {
    Refusal::Store { error: format!("the answer to {call:?} was for another kind of call") }
}

/// Whether a group was paused before the first of `entries`: the opposite of what the first pause
/// or resume among them did, since each changes it, or with neither, as it is after them.
fn paused_before(entries: &[Entry], after: bool) -> bool {
    let first = entries.iter().find_map(|entry| match &entry.what {
        What::Changed { change: Change::Paused, .. } => Some(false),
        What::Changed { change: Change::Resumed, .. } => Some(true),
        _ => None,
    });
    first.unwrap_or(after)
}

/// Keeps only the last of `wakes` that tells the windows what waits for the human: it holds the
/// whole batch, and one per message would raise the notice again for each (MIP-4, section 10).
fn keep_last_human_wake(wakes: &mut Vec<Wake>) {
    if let Some(last) = wakes.iter().rposition(|wake| wake.via == Via::Human) {
        let mut index = 0;
        wakes.retain(|wake| {
            index += 1;
            wake.via != Via::Human || index - 1 == last
        });
    }
}
