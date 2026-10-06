//! Messages between agents (MIP-4), hosted: `muster-msg`'s service over this daemon's disk and
//! Claude Code's inbox sockets, answering `msg` requests.
//!
//! It has a lock of its own rather than the session's, because a post waits on the disk and a
//! wait on another agent, and neither has anything to do with panes. Wakes are delivered with
//! that lock let go, so a session slow to take one delays only the post that woke it.

mod carry;
mod command;
mod compacts;
mod doorbell;
mod inbox;
pub(crate) mod peer;
mod presence;
mod prompt;
mod renames;
mod store;
mod typist;
mod wire;

pub(crate) use doorbell::Doorbell;
pub(crate) use peer::Peers;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_daemon_proto::messaging::{self, GROUP, JOIN, LOG, READ, WHO};
use muster_msg::{
    Action, Activity, AnsweredWait, Away, Caller, Change, Changed, Draft, Entry, Found, Inbox,
    LARGEST_BODY, LONGEST_GROUP, Liveness, Messaging, Policy, Presence, Reach, Refusal, Route,
    Settled, Tell, Via, Wake, What,
};
use proto::answer::Detail;
use proto::msg_answer::{self, Answer};
use proto::msg_request::Request as Asked;

use crate::session::{HANDING_OVER, Reply, Shared};
use carry::Destination;
use doorbell::{By, Now};
use presence::Panes;
use store::Files;

/// How often a wait looks up from its channel to see whether its caller hung up.
pub(crate) const LOOK_UP: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub(crate) struct Messages {
    service: Messaging<Files>,
    /// A handoff is under way: what is kept on disk is what the new daemon will read, so
    /// nothing may change it.
    handing_over: bool,
    /// Waits in progress, by ticket, and how to end each.
    waits: HashMap<u64, Sender<WaitEnded>>,
    /// Wakes for agents in panes that could not be rung yet, which the doorbell rings.
    pending: Vec<Wake>,
    /// Rings their agents have not yet taken, which the doorbell presses Return for again.
    rung: Vec<doorbell::Rung>,
    /// Wakes handed to their sessions by command that their agents have not yet taken.
    commanded: Vec<doorbell::Commanded>,
    /// Panes whose wake command is running. Nothing else is typed or run there until it ends.
    commanding: HashSet<String>,
    /// Wakes whose command took too long, by participant and group, to be typed instead.
    type_instead: HashSet<(String, String)>,
    /// Panes whose wake command has taken too long once: a second time ends the route.
    stalled: HashSet<String>,
    /// Agents whose hooks fetch their messages, seen idle with none fetching, and when the
    /// doorbell may ring them: a `Stop` hook's wait may connect just after its turn ends.
    hook_grace: HashMap<String, Instant>,
    /// Rings given up with their text possibly left unsent in a prompt, by pane: the text, and
    /// when it was typed. A later ring finding exactly that in the prompt sends it.
    left: HashMap<String, (String, Instant)>,
    /// What waits for the human, changed by the request being handled, for the windows to be
    /// told once this lock is let go.
    told: Vec<msg_answer::Notice>,
    /// How many times the human's notices have been handed out, which orders them.
    tellings: u64,
    /// The groups the human is in, as the windows were last told: one joined since is told of
    /// with what waits there, so a window lists it before anything is posted to it.
    human_groups: BTreeSet<String>,
    /// Follows of a log waiting for its next entry, woken by anything that may have appended.
    follows: Vec<Sender<()>>,
    /// Changes made by the request being handled, for the other machines with members in
    /// their groups to be sent once this lock is let go.
    telling: Vec<Tell>,
}

#[derive(Debug)]
enum WaitEnded {
    Ready(Vec<msg_answer::Notice>),
    Superseded,
    /// Its participant left, so nothing will answer it.
    Left,
}

impl Messages {
    /// Whatever the store beside `socket` holds, or nothing.
    pub(crate) fn load(socket: &Path) -> Messages {
        let files = Files::beside(socket);
        let found = files.load();
        let service = Messaging::restore(files, found.saved, found.logs);
        // A window that attends this daemon is told these by its snapshot.
        let human_groups = service.human_groups();
        Messages {
            // A wake the daemon before this one deferred and never rang is lost with it, and one
            // it rang cannot be told from one it did not, so each is rung again once its pane
            // allows: at worst a wake too many.
            pending: service.outstanding(),
            rung: Vec::new(),
            commanded: Vec::new(),
            commanding: HashSet::new(),
            type_instead: HashSet::new(),
            stalled: HashSet::new(),
            hook_grace: HashMap::new(),
            left: HashMap::new(),
            service,
            handing_over: false,
            waits: HashMap::new(),
            told: Vec::new(),
            tellings: 0,
            human_groups,
            follows: Vec::new(),
            telling: Vec::new(),
        }
    }

    /// What waits for the human, in each group with anything waiting.
    pub(crate) fn human(&self) -> Vec<msg_answer::Notice> {
        self.service.human_notices().iter().map(|notice| self.for_human(notice)).collect()
    }

    /// `notice` as the windows are told it, saying whether the human is still in its group.
    fn for_human(&self, notice: &muster_msg::Notice) -> msg_answer::Notice {
        msg_answer::Notice { member: self.service.human_is_in(&notice.group), ..notice_of(notice) }
    }

    /// What the request just handled changed for the human, in the order the service decided
    /// it, for [`tell_human`] once this lock is let go.
    fn take_told(&mut self) -> Told {
        let groups = self.service.human_groups();
        for group in groups.difference(&self.human_groups) {
            self.told.push(self.for_human(&self.service.human_notice(group)));
        }
        self.human_groups = groups;
        self.tellings += 1;
        Told { order: self.tellings, notices: std::mem::take(&mut self.told) }
    }

    /// Wakes every follow of a log, each to look for entries of its own group.
    fn appended(&mut self) {
        for follow in self.follows.drain(..) {
            let _ = follow.send(());
        }
    }

    /// Marks a handoff as under way, or as over because it failed. Taken under this lock, so a
    /// change already begun has been kept by the time the new daemon reads the store. The
    /// waits end, and their callers ask again of whichever daemon serves next.
    pub(crate) fn handing_over(&mut self, underway: bool) {
        self.handing_over = underway;
        // Dropped, so each follow ends and asks whichever daemon serves next.
        self.follows.clear();
        if underway {
            for ticket in self.waits.keys() {
                self.service.cancel_wait(*ticket);
            }
            self.waits.clear();
        } else {
            // The handover failed. A new daemon that took over and died before this one
            // resumed may have written to the store, so read back what it holds.
            let files = self.service.store().clone();
            let found = files.load();
            self.service = Messaging::restore(files, found.saved, found.logs);
            self.human_groups = self.service.human_groups();
            // Every wake still unread is rung again, so a ring from before is not also pressed.
            self.rung.clear();
            self.pending = self.service.outstanding();
        }
    }

    /// A group's members changed: ends the waits of those removed, and tells the windows
    /// nothing waits for the human if it was.
    fn members_changed(&mut self, members: Changed) -> (String, Answer) {
        for ticket in &members.ended {
            if let Some(wait) = self.waits.remove(ticket) {
                let _ = wait.send(WaitEnded::Left);
            }
        }
        // Removed is the human leaving, as far as the windows are concerned.
        if members.removed.iter().any(|name| name == muster_msg::HUMAN) {
            self.told.push(nothing_waits(&members.group, false));
        }
        changed("members", members, &mut self.telling)
    }

    fn answer(&mut self, caller: &Caller, asked: Asked, panes: &Panes) -> Reply {
        let changes = !matches!(asked, Asked::Who(_) | Asked::Log(_) | Asked::Groups(_));
        if changes && self.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let result = match asked {
            Asked::Who(who) => self.service.who(who.group.as_deref(), panes).map(|members| {
                let members = members.into_iter().map(member_of).collect();
                (String::new(), Answer::Members(msg_answer::Members { members }))
            }),
            Asked::Read(read) => {
                let told = &mut self.told;
                self.service.read(caller, read.group.as_deref(), panes).map(|read| {
                    if read.name == muster_msg::HUMAN {
                        told.extend(
                            read.groups.iter().map(|(group, _)| nothing_waits(group, true)),
                        );
                    }
                    let groups = read
                        .groups
                        .into_iter()
                        .map(|(group, entries)| msg_answer::GroupEntries {
                            behind: self.service.behind(&group).map(str::to_string),
                            group,
                            entries: entries.iter().map(entry_of).collect(),
                        })
                        .collect();
                    (read.name, Answer::Entries(msg_answer::Entries { groups }))
                })
            }
            Asked::Log(asked) => self.service.log(&asked.group, asked.since).map(|entries| {
                let group = msg_answer::GroupEntries {
                    behind: self.service.behind(&asked.group).map(str::to_string),
                    group: asked.group,
                    entries: entries.iter().map(entry_of).collect(),
                };
                (String::new(), Answer::Entries(msg_answer::Entries { groups: vec![group] }))
            }),
            Asked::Groups(_) | Asked::GroupNew(_) | Asked::GroupSet(_) | Asked::Pause(_) => {
                self.group(caller, asked, panes)
            }
            Asked::Join(_)
            | Asked::GroupMembers(_)
            | Asked::Leave(_)
            | Asked::Post(_)
            | Asked::Wait(_)
            | Asked::Resume(_)
            | Asked::GroupDelete(_)
            | Asked::Peer(_) => unreachable!("each of these is handled apart"),
        };
        match result {
            Ok((caller, answer)) => answered(caller, answer),
            Err(refusal) => refused("", &refusal),
        }
    }

    /// A group made or changed, or the groups listed (MIP-4, section 8).
    fn group(
        &mut self,
        caller: &Caller,
        asked: Asked,
        panes: &Panes,
    ) -> Result<(String, Answer), Refusal> {
        let telling = &mut self.telling;
        match asked {
            Asked::Groups(_) => {
                let groups = self
                    .service
                    .groups()
                    .into_iter()
                    .map(|group| msg_answer::Group {
                        name: group.name,
                        members: group.members,
                        policy: Some(policy_of(&group.policy)),
                    })
                    .collect();
                Ok((String::new(), Answer::Groups(msg_answer::Groups { groups })))
            }
            Asked::GroupNew(new) => {
                let policy = new.policy.map(policy_from);
                self.service
                    .group_new(caller, &new.group, policy, panes, now_ms())
                    .map(|made| changed("made", made, telling))
            }
            Asked::GroupSet(set) => {
                let asked = set.policy.unwrap_or_default();
                // A client or a machine from before 1.3 says nothing of urgent posts; leaving
                // them as they were keeps a restriction it cannot see from being lifted.
                let unsaid = asked.urgent.is_none();
                let mut policy = policy_from(asked);
                if unsaid && let Some(kept) = self.service.policy(&set.group) {
                    policy.urgent.clone_from(&kept.urgent);
                }
                let changed_it =
                    self.service.group_set(caller, &set.group, policy, panes, now_ms())?;
                // The policy decides which messages ring the human, so what waits may have moved.
                self.told.push(self.for_human(&self.service.human_notice(&changed_it.group)));
                Ok(changed("set_policy", changed_it, &mut self.telling))
            }
            Asked::Pause(pause) => self
                .service
                .pause(caller, &pause.group, panes, now_ms())
                .map(|paused| changed("paused", paused, telling)),
            _ => unreachable!("only group requests are answered here"),
        }
    }
}

/// Answers a `msg` request. `hung_up` says whether the caller has gone, for a wait that would
/// otherwise outlive it.
///
/// A person's request this machine refuses because the human is homed elsewhere is carried
/// there, when a link to it is up, and answered as that machine answers it (MIP-4, section 10).
/// So is the human's own delete, pause or resume of a group kept on another machine, to that
/// machine.
pub(crate) fn handle(
    shared: &Arc<Shared>,
    request: proto::MsgRequest,
    hung_up: &dyn Fn() -> bool,
) -> Reply {
    let caller = caller_of(request.caller.unwrap_or_default());
    let Some(asked) = request.request else { return Reply::unsupported() };
    if let Asked::Peer(peer) = &asked {
        return peer::hold(shared, &peer.name, &peer.socket, hung_up);
    }
    let panes = Panes::of(shared);
    let carriable = carry::may_carry(&shared.messages().service, &caller, &asked, &panes);
    let kept = carriable.then(|| asked.clone());
    let reply = respond(shared, &caller, asked, hung_up, &panes);
    let reply = match kept {
        None => reply,
        Some(asked) => match carry::destination(shared, &caller, &asked, &reply, &panes) {
            None => reply,
            Some(Destination::HumanHome(home)) => {
                let asked = carry::outward(&shared.messages().service, asked, &panes);
                peer::carry(shared, &home.machine, asked, hung_up).unwrap_or(reply)
            }
            Some(Destination::GroupHome(machine)) => {
                let asked = carry::outward(&shared.messages().service, asked, &panes);
                peer::carry(shared, &machine, asked, hung_up)
                    .map(|carried| peer::named_here(shared, &machine, carried))
                    .unwrap_or(reply)
            }
        },
    };
    naming_the_human(shared, reply)
}

/// The answer, saying what to call the human where it is shown (MIP-4, section 10). Read from
/// the session, which keeps it with the app's other settings, and never with the messages lock
/// held.
fn naming_the_human(shared: &Shared, mut reply: Reply) -> Reply {
    if let Some(Detail::Msg(answer)) = reply.detail.as_deref_mut()
        && let Some(name) = shared.lock().human_name()
    {
        name.clone_into(&mut answer.human_name);
    }
    reply
}

/// A request the person made on `peer`'s machine, carried here to be done as the human.
pub(crate) fn carried(
    shared: &Arc<Shared>,
    peer: &muster_msg::Peer,
    asked: Asked,
    hung_up: &dyn Fn() -> bool,
) -> Reply {
    let asked = carry::inward(peer, asked);
    log::info("msg.carried", fields! { "machine" => peer.name, "verb" => carry::verb(&asked) });
    let panes = Panes::of(shared);
    let human = Caller { at_ms: now_ms(), ..Caller::default() };
    respond(shared, &human, asked, hung_up, &panes)
}

/// Answers `asked` on this machine, as `caller`.
fn respond(
    shared: &Arc<Shared>,
    caller: &Caller,
    asked: Asked,
    hung_up: &dyn Fn() -> bool,
    panes: &Panes,
) -> Reply {
    match asked {
        Asked::Post(post) => posting(shared, caller, &post, panes),
        Asked::Resume(resume) => resuming(shared, caller, &resume.group, panes),
        Asked::Wait(wait) => waiting(shared, caller, &wait, hung_up, panes),
        Asked::Log(log) if log.follow => following(shared, log, hung_up),
        Asked::Join(join) => joining(shared, caller, &join, panes),
        Asked::Leave(leave) => leaving(shared, caller, &leave, panes),
        Asked::Who(who) => whoing(shared, &who, panes),
        Asked::GroupMembers(members) => membering(shared, caller, &members, panes),
        Asked::GroupDelete(delete) => deleting(shared, caller, &delete.group, panes),
        Asked::Peer(_) => {
            refused_as("", "not_carried", "a link is held by the daemon it is asked of")
        }
        asked => {
            let (reply, told, telling) = {
                let mut messages = shared.messages();
                let reply = messages.answer(caller, asked, panes);
                messages.appended();
                (reply, messages.take_told(), std::mem::take(&mut messages.telling))
            };
            tell_human(shared, told);
            peer::tell(shared, &telling);
            reply
        }
    }
}

/// Deletes a group kept here (MIP-4, section 8). Its follows are woken to find it gone, the
/// waits kept to it end, the windows hear nothing waits for the human there, and each machine
/// with a member is told to forget its replica.
fn deleting(shared: &Arc<Shared>, caller: &Caller, group: &str, panes: &Panes) -> Reply {
    let (result, told) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let result = messages.service.group_delete(caller, group, panes);
        if let Ok(deleted) = &result {
            messages.let_go(&deleted.ended);
            messages.told.push(nothing_waits(&deleted.group, false));
            messages.appended();
        }
        (result, messages.take_told())
    };
    tell_human(shared, told);
    let deleted = match result {
        Ok(deleted) => deleted,
        Err(refusal) => return refused("", &refusal),
    };
    if let Some(error) = &deleted.unsaved {
        kept_nothing(&Refusal::Store { error: error.clone() });
    }
    log::info(
        "msg.group.deleted",
        fields! {
            "by" => deleted.by,
            "group" => deleted.group,
            "entries" => deleted.entries,
            "let_go" => deleted.let_go.join(","),
        },
    );
    peer::forget(shared, &deleted.forget, &deleted.group);
    let answer = Answer::Deleted(msg_answer::Deleted {
        group: deleted.group,
        entries: deleted.entries,
        let_go: deleted.let_go,
    });
    answered(deleted.by, answer)
}

/// What waits for the human, as the service decided it, for the windows to be told.
#[derive(Debug, Default)]
pub(crate) struct Told {
    order: u64,
    notices: Vec<msg_answer::Notice>,
}

/// Tells the windows attending this daemon what waits for the human now. The session is
/// locked here and never with the messages lock held, so [`Told::order`] is what keeps two
/// requests that raced to this point from leaving the older word standing.
fn tell_human(shared: &Shared, told: Told) {
    if !told.notices.is_empty() {
        shared.lock().tell_human(told.order, told.notices);
    }
}

/// Nothing waits for the human in `group`, which they are still `member` of, or have left.
fn nothing_waits(group: &str, member: bool) -> msg_answer::Notice {
    msg_answer::Notice { group: group.to_string(), member, ..msg_answer::Notice::default() }
}

/// Answers a follow of a log once its group has an entry after `since`, at once if it already
/// does. It is woken by whatever may have appended, and looks up every [`LOOK_UP`] to see
/// whether its caller has gone.
fn following(shared: &Shared, log: proto::msg_request::Log, hung_up: &dyn Fn() -> bool) -> Reply {
    loop {
        let woken = {
            let mut messages = shared.messages();
            if messages.handing_over {
                return ended_by_handover();
            }
            match messages.service.log(&log.group, log.since) {
                Err(refusal) => return refused("", &refusal),
                Ok(entries) if !entries.is_empty() => {
                    let group = msg_answer::GroupEntries {
                        group: log.group,
                        entries: entries.iter().map(entry_of).collect(),
                        behind: None,
                    };
                    let groups = vec![group];
                    return answered(
                        String::new(),
                        Answer::Entries(msg_answer::Entries { groups }),
                    );
                }
                Ok(_) => {}
            }
            let (wake, woken) = mpsc::channel();
            messages.follows.push(wake);
            log::debug("msg.following", fields! { "since" => log.since });
            woken
        };
        loop {
            match woken.recv_timeout(LOOK_UP) {
                Ok(()) => break,
                Err(RecvTimeoutError::Disconnected) => return ended_by_handover(),
                Err(RecvTimeoutError::Timeout) if hung_up() => {
                    return refused_as("", "timed_out", "the caller hung up");
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

fn ended_by_handover() -> Reply {
    refused_as("", "ended", "the daemon is handing over; ask again")
}

/// Wakes a change made, sorted under the service's lock to be delivered once it is let go.
#[derive(Debug, Default)]
pub(crate) struct Holding {
    sending: Vec<Wake>,
    ringing: Vec<(Wake, presence::Seen)>,
    deferred: Vec<(String, msg_answer::Until)>,
    /// What waits for the human now, for the windows attending this daemon.
    told: Told,
}

/// What delivering a [`Holding`] came to: who could not be woken, and who is left for the
/// doorbell and until what.
#[derive(Debug, Default)]
struct Rang {
    failed: Vec<String>,
    deferred: Vec<(String, msg_answer::Until)>,
}

impl Messages {
    /// Whether `wake` is to be typed, its command having taken too long.
    fn types(&self, wake: &Wake) -> bool {
        self.type_instead.contains(&(wake.name.clone(), wake.notice.group.clone()))
    }

    /// Ends each answered wait with what it was told.
    pub(crate) fn end_waits(&mut self, answered: &[AnsweredWait]) {
        for answered in answered {
            if let Some(wait) = self.waits.remove(&answered.ticket) {
                let _ = wait.send(WaitEnded::Ready(vec![notice_of(&answered.notice)]));
            }
        }
    }

    /// Ends the waits of members a group kept elsewhere let go of, as a leave ends its own.
    pub(crate) fn let_go(&mut self, tickets: &[u64]) {
        for ticket in tickets {
            if let Some(wait) = self.waits.remove(ticket) {
                let _ = wait.send(WaitEnded::Left);
            }
        }
    }

    /// Ends the waits a change answered and sorts its wakes: sent now, rung now, or left for the
    /// doorbell. Under the lock, so a wait's answer and the service's record of it cannot part.
    pub(crate) fn hold(
        &mut self,
        wakes: &[Wake],
        answered: &[AnsweredWait],
        panes: &Panes,
    ) -> Holding {
        self.end_waits(answered);
        let now = Instant::now();
        let mut holding = Holding::default();
        for wake in wakes {
            let pane = match &wake.via {
                Via::Pane(pane) => pane,
                Via::Human => {
                    self.told.push(self.for_human(&wake.notice));
                    continue;
                }
                Via::Inbox(_) => {
                    holding.sending.push(wake.clone());
                    continue;
                }
            };
            // A later wake for a group stands for any earlier one still waiting to ring: its
            // notice covers everything unread there. Only an urgent post makes a second.
            self.pending.retain(|earlier| {
                earlier.name != wake.name || earlier.notice.group != wake.notice.group
            });
            // A pane whose wake command is running is left to the doorbell, which that command
            // nudges as it ends.
            if self.commanding.contains(pane) {
                holding.deferred.push((wake.name.clone(), msg_answer::Until::Prompt));
                self.pending.push(wake.clone());
                continue;
            }
            // A pane whose agent has not been found yet waits for it like a busy one.
            let urgent = doorbell::is_urgent(wake);
            let ringing = panes
                .get(pane)
                .map(|seen| (doorbell::reach(seen, now, urgent, None, self.types(wake)), seen));
            let until = match ringing {
                // The doorbell starts the command, nudged once this lock is let go.
                Some((By::Command, _)) => {
                    self.pending.push(wake.clone());
                    continue;
                }
                Some((By::Typing(Now::Ring), seen)) => {
                    holding.ringing.push((wake.clone(), seen.clone()));
                    continue;
                }
                Some((By::Typing(Now::AtIdle), _)) => msg_answer::Until::Idle,
                Some((By::Typing(Now::Unblocked), _)) => msg_answer::Until::Unblocked,
                Some((By::Typing(Now::At(_)), _)) => msg_answer::Until::Prompt,
                None => msg_answer::Until::Agent,
            };
            holding.deferred.push((wake.name.clone(), until));
            self.pending.push(wake.clone());
        }
        self.appended();
        holding.told = self.take_told();
        holding
    }
}

/// Delivers what [`Messages::hold`] sorted, with the service's lock let go.
fn ring(shared: &Shared, holding: Holding) -> Rang {
    let Holding { sending, ringing, mut deferred, told } = holding;
    tell_human(shared, told);
    let names: Vec<(String, String)> =
        ringing.iter().map(|(wake, _)| (wake.name.clone(), wake.notice.group.clone())).collect();
    let came = doorbell::ring_all(shared, ringing);
    let mut refused_rings: Vec<(String, String)> = Vec::new();
    for ((name, group), came) in names.into_iter().zip(came) {
        match came {
            doorbell::Came::Rang => {}
            doorbell::Came::Waits => deferred.push((name, msg_answer::Until::Prompt)),
            doorbell::Came::Refused => refused_rings.push((name, group)),
        }
    }
    // Rung, or left for the doorbell: either way it has something to look at.
    shared.doorbell.nudge();
    let failed = wake(&sending);
    if !failed.is_empty() || !refused_rings.is_empty() {
        let mut messages = shared.messages();
        if !messages.handing_over {
            for wake in &failed {
                if let Err(refusal) = messages.service.delivered(wake, false) {
                    kept_nothing(&refusal);
                }
            }
            // A pane that would not take the ring is no reason to think its agent gone, but
            // the agent was not woken, and the next post should try again.
            for (name, group) in &refused_rings {
                if let Some(error) = messages.service.unwake(name, group) {
                    kept_nothing(&Refusal::Store { error });
                }
            }
        }
    }
    let failed = failed
        .iter()
        .map(|wake| wake.name.clone())
        .chain(refused_rings.into_iter().map(|(name, _)| name))
        .collect();
    Rang { failed, deferred }
}

fn joining(
    shared: &Arc<Shared>,
    caller: &Caller,
    join: &proto::msg_request::Join,
    panes: &Panes,
) -> Reply {
    let (name, group) = (join.name.as_deref(), join.group.as_deref());
    let route = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        messages.service.route_join(caller, name, group, panes)
    };
    let route = match route {
        Ok(Route::Ask { group }) => ask_around(shared, caller, name, &group, panes),
        other => other,
    };
    let joined = joined_by(shared, caller, name, group, route, panes);
    // A join that renamed the caller did so whether or not the group took it.
    send_folded(shared);
    let joined = match joined {
        Ok(joined) => joined,
        Err(reply) => return reply,
    };
    // Its hooks fetch its messages, wherever the group it joined is kept (MIP-4, section 6).
    if join.pull
        && let Err(refusal) = shared.messages().service.pulls(&joined.name)
    {
        return refused("", &refusal);
    }
    log::info(
        "msg.joined",
        fields! {
            "name" => joined.name,
            "group" => joined.group.clone().unwrap_or_default(),
            "created" => joined.created,
            "took_over" => joined.took_over,
        },
    );
    let answer = Answer::Joined(msg_answer::Joined {
        name: joined.name.clone(),
        group: joined.group,
        created: joined.created,
        took_over: joined.took_over,
        groups: joined.groups,
    });
    answered(joined.name, answer)
}

/// Joins where `route` says: through the group's home, or here.
fn joined_by(
    shared: &Arc<Shared>,
    caller: &Caller,
    name: Option<&str>,
    group: Option<&str>,
    route: Result<Route, Refusal>,
    panes: &Panes,
) -> Result<muster_msg::Joined, Reply> {
    match route {
        Err(refusal) => Err(refused("", &refusal)),
        Ok(Route::Away(away)) => {
            let (settle, holding) = peer::call_away(shared, &away);
            ring(shared, holding);
            match settle.result {
                Ok(Settled::Joined(joined)) => Ok(joined),
                Ok(other) => Err(mismatched("join", &other)),
                Err(refusal) => Err(refused("", &refusal)),
            }
        }
        Ok(_) => {
            let joined = {
                let mut messages = shared.messages();
                if messages.handing_over {
                    return Err(refused_as("", "handing_over", HANDING_OVER));
                }
                let joined = messages.service.join(caller, name, group, panes, now_ms());
                messages.appended();
                joined
            };
            let joined = joined.map_err(|refusal| refused("", &refusal))?;
            peer::tell(shared, &joined.tell);
            let told = shared.messages().take_told();
            tell_human(shared, told);
            Ok(joined)
        }
    }
}

/// Sends what a join's folding a pane into a name left for other machines: the entries it
/// appended to groups kept here, and the joins that put the name in the pane's place in groups
/// kept elsewhere.
fn send_folded(shared: &Arc<Shared>) {
    let folded = {
        let mut messages = shared.messages();
        messages.appended();
        messages.service.take_folded()
    };
    peer::tell(shared, &folded.tell);
    for away in folded.away {
        let (settle, holding) = peer::call_away(shared, &away);
        ring(shared, holding);
        if let Err(refusal) = settle.result {
            log::warn(
                "msg.fold.refused",
                fields! {
                    "machine" => away.machine,
                    "group" => away.call.group(),
                    "why" => words(&refusal),
                    "impact" => "the group still lists the agent by its pane's name there, and \
                                 posts to that name find nobody here to wake",
                    "check" => "whether that machine's daemon speaks a protocol before 1.4, which \
                                takes this as an ordinary join its policy may refuse, or the \
                                link dropped; there, `group add` adds the agent by its new name",
                },
            );
        }
    }
}

/// Asks each linked machine whether it keeps a group by this name, and joins it there if one
/// does; here, creating it, if none does.
fn ask_around(
    shared: &Arc<Shared>,
    caller: &Caller,
    name: Option<&str>,
    group: &str,
    panes: &Panes,
) -> Result<Route, Refusal> {
    let mut keeping = Vec::new();
    for machine in shared.peers.machines() {
        let call = muster_msg::Call::Find { group: group.to_string() };
        let (settle, _) = peer::call_away(shared, &Away { machine: machine.clone(), call });
        if matches!(settle.result, Ok(Settled::Found(true))) {
            keeping.push(format!("{group}@{machine}"));
        }
    }
    match keeping.as_slice() {
        [] => {
            shared.messages().service.unchecked(group)?;
            Ok(Route::Here)
        }
        [there] => shared.messages().service.route_join(caller, name, Some(there), panes),
        _ => Err(Refusal::WhichGroup { candidates: keeping }),
    }
}

fn leaving(
    shared: &Arc<Shared>,
    caller: &Caller,
    leave: &proto::msg_request::Leave,
    panes: &Panes,
) -> Reply {
    let group = leave.group.as_deref();
    let away = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        messages.service.route_leave(caller, group, panes)
    };
    let away = match away {
        Ok(away) => away,
        Err(refusal) => return refused("", &refusal),
    };
    let (mut name, mut groups) = (String::new(), Vec::new());
    for away in &away {
        let (settle, holding) = peer::call_away(shared, away);
        ring(shared, holding);
        match settle.result {
            Ok(Settled::Left(left)) => {
                end_wait(shared, left.ended);
                name = left.name;
                groups.extend(left.groups);
            }
            Ok(other) => return mismatched("leave", &other),
            Err(refusal) => return refused("", &refusal),
        }
    }
    let mut stopped = false;
    if group.is_none() || away.is_empty() {
        let left = {
            let mut messages = shared.messages();
            if messages.handing_over {
                return refused_as("", "handing_over", HANDING_OVER);
            }
            let left = messages.service.leave(caller, group, panes, now_ms());
            messages.appended();
            left
        };
        match left {
            Ok(left) => {
                end_wait(shared, left.ended);
                peer::tell(shared, &left.tell);
                name = left.name;
                groups.extend(left.groups);
                stopped = left.stopped;
            }
            Err(refusal) => return refused("", &refusal),
        }
    }
    if name == muster_msg::HUMAN {
        let told = {
            let mut messages = shared.messages();
            messages.told.extend(groups.iter().map(|group| nothing_waits(group, false)));
            messages.take_told()
        };
        tell_human(shared, told);
    }
    log::info(
        "msg.left",
        fields! { "name" => name, "groups" => groups.join(","), "stopped" => stopped },
    );
    answered(name, Answer::Left(msg_answer::Left { groups, stopped }))
}

fn end_wait(shared: &Shared, ticket: Option<u64>) {
    let wait = ticket.and_then(|ticket| shared.messages().waits.remove(&ticket));
    if let Some(wait) = wait {
        let _ = wait.send(WaitEnded::Left);
    }
}

/// Members of a group, or every participant here: a group's members on another machine are
/// as that machine's daemon says, asked now.
fn whoing(shared: &Arc<Shared>, who: &proto::msg_request::Who, panes: &Panes) -> Reply {
    let group = who.group.as_deref();
    let found = {
        let messages = shared.messages();
        messages
            .service
            .who(group, panes)
            .and_then(|members| Ok((members, messages.service.route_who(group)?)))
    };
    let (mut members, away) = match found {
        Ok(found) => found,
        Err(refusal) => return refused("", &refusal),
    };
    for away in away {
        let (settle, holding) = peer::call_away(shared, &away);
        ring(shared, holding);
        if let Ok(Settled::Members(heard)) = settle.result {
            muster_msg::heard(&mut members, &away.machine, &heard);
        }
    }
    let members = members.into_iter().map(member_of).collect();
    answered(String::new(), Answer::Members(msg_answer::Members { members }))
}

/// A reply from another machine that settled as another kind of answer than the call asked
/// for: two daemons disagreeing about the protocol, which is a bug.
fn mismatched(asked: &str, settled: &Settled) -> Reply {
    log::error(
        "msg.peer.mismatched",
        fields! {
            "asked" => asked,
            "settled" => format!("{settled:?}"),
            "impact" => "the request was refused, though the other machine may have done it",
            "check" => "whether both daemons are the same build; this is a bug if they are",
        },
    );
    refused_as("", "mismatched", &format!("the other machine answered a {asked} with {settled:?}"))
}

fn posting(
    shared: &Arc<Shared>,
    caller: &Caller,
    post: &proto::msg_request::Post,
    panes: &Panes,
) -> Reply {
    let routed = finding(shared, |found| {
        let mut messages = shared.messages();
        if messages.handing_over {
            return Ok(None);
        }
        messages.service.route_post(caller, &draft_of(post, found), panes).map(Some)
    });
    let (route, found) = match routed {
        Err(refusal) => return refused("", &refusal),
        Ok((None, _)) => return refused_as("", "handing_over", HANDING_OVER),
        Ok((Some(route), found)) => (route, found),
    };
    let delivered = if let Route::Away(away) = route {
        if let Some(refusal) = urgent_unknown_at(shared, post, &away) {
            return refusal;
        }
        let (settle, holding) = peer::call_away(shared, &away);
        let rang = ring(shared, holding);
        match settle.result {
            Ok(Settled::Posted(posted)) => {
                let activities = activities_of(shared, &posted, panes);
                Delivered { posted, rang, activities }
            }
            Ok(other) => return mismatched("post", &other),
            Err(refusal) => return refused("", &refusal),
        }
    } else {
        let delivered = delivering(shared, panes, |service| {
            service.post_draft(caller, &draft_of(post, &found), panes, now_ms())
        });
        match delivered {
            Ok(delivered) => delivered,
            Err(reply) => return reply,
        }
    };
    let posted = told("msg.posted", Some(post.body.len()), &delivered);
    answered(delivered.posted.author.clone(), Answer::Posted(posted))
}

/// The first protocol whose daemons post urgently. An older one ignores the field it does not
/// know, so an urgent post sent to it would ring as an ordinary one.
const URGENT_SINCE: u32 = 3;

/// A refusal for an urgent post bound for a machine whose daemon cannot post one.
fn urgent_unknown_at(
    shared: &Shared,
    post: &proto::msg_request::Post,
    away: &Away,
) -> Option<Reply> {
    let speaks = shared.peers.to(&away.machine)?.speaks;
    let (code, words) = urgent_unsupported(post.urgent, speaks, away.call.group(), &away.machine)?;
    Some(refused_as("", code, &words))
}

/// Its own code rather than `not_urgent`, which is a group's policy saying no: this one is
/// answered by updating a machine, not by asking whoever may.
fn urgent_unsupported(
    urgent: bool,
    speaks: proto::Version,
    group: &str,
    machine: &str,
) -> Option<(&'static str, String)> {
    (urgent && speaks.minor < URGENT_SINCE).then(|| {
        let words = format!(
            "{group} is kept on {machine}, whose muster-daemon speaks protocol {speaks}, which \
             cannot post urgently. Update Muster there, or post without --urgent"
        );
        ("urgent_unsupported", words)
    })
}

fn draft_of<'a>(post: &'a proto::msg_request::Post, found: &'a [Found]) -> Draft<'a> {
    Draft {
        group: post.group.as_deref(),
        to: &post.to,
        found,
        body: &post.body,
        urgent: post.urgent,
    }
}

/// Adds and removes a group's members, finding a name added that means nobody here on the
/// machines linked to this one.
fn membering(
    shared: &Arc<Shared>,
    caller: &Caller,
    members: &proto::msg_request::GroupMembers,
    panes: &Panes,
) -> Reply {
    let result = finding(shared, |found| {
        let mut messages = shared.messages();
        if messages.handing_over {
            return Ok(None);
        }
        let (group, add, remove) = (&members.group, &members.add, &members.remove);
        let changed =
            messages.service.group_members(caller, group, add, remove, found, panes, now_ms())?;
        let answer = messages.members_changed(changed);
        messages.appended();
        Ok(Some((answer, messages.take_told(), std::mem::take(&mut messages.telling))))
    });
    match result {
        Err(refusal) => refused("", &refusal),
        Ok((None, _)) => refused_as("", "handing_over", HANDING_OVER),
        Ok((Some(((by, answer), told, telling)), _)) => {
            tell_human(shared, told);
            peer::tell(shared, &telling);
            answered(by, answer)
        }
    }
}

/// Runs `act` with the names other machines said they have, starting with none. Each time it
/// is refused for a name that means nobody here, asks the machines linked to this one whom
/// that name means there (MIP-4, section 11) and runs it again, until it is refused for a name
/// already asked about, or for anything else. Returns what was found with the outcome, for a
/// later step of the same request to take.
fn finding<T>(
    shared: &Shared,
    mut act: impl FnMut(&[Found]) -> Result<T, Refusal>,
) -> Result<(T, Vec<Found>), Refusal> {
    let mut found = Vec::new();
    let mut asked = BTreeSet::new();
    loop {
        match act(&found) {
            Err(Refusal::NoSuchParticipant { name }) if asked.insert(name.clone()) => {
                let there = whom(shared, &name);
                if there.is_empty() {
                    return Err(Refusal::NoSuchParticipant { name });
                }
                found.extend(there.into_iter().map(|there| Found { name: name.clone(), there }));
            }
            Err(refusal) => return Err(refusal),
            Ok(outcome) => return Ok((outcome, found)),
        }
    }
}

/// Whom `name` means on each machine linked to this one, as this machine writes it: only the
/// machine it names, for `name@machine`.
fn whom(shared: &Shared, name: &str) -> Vec<String> {
    let (base, machines) = match muster_msg::split_machine(name) {
        Some((base, machine)) => (base, vec![machine.to_string()]),
        None => (name, shared.peers.machines()),
    };
    let call = muster_msg::Call::Whom { name: base.to_string() };
    if call.check().is_err() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for machine in machines {
        let (settle, _) = peer::call_away(shared, &Away { machine, call: call.clone() });
        if let Ok(Settled::Named(Some(there))) = settle.result {
            found.push(there);
        }
    }
    found
}

/// A resume wakes as a post does, and is answered as one.
fn resuming(shared: &Shared, caller: &Caller, group: &str, panes: &Panes) -> Reply {
    let delivered =
        delivering(shared, panes, |service| service.resume(caller, group, panes, now_ms()));
    match delivered {
        Ok(delivered) => {
            let resumed = told("msg.resumed", None, &delivered);
            answered(delivered.posted.author.clone(), Answer::Resumed(resumed))
        }
        Err(reply) => reply,
    }
}

/// What delivering a post's wakes came to.
struct Delivered {
    posted: muster_msg::Posted,
    rang: Rang,
    activities: HashMap<String, Activity>,
}

/// Runs `act` under the messaging lock, then delivers the wakes it returns with the lock let
/// go - rung, sent to an inbox, answered to a wait, or left for the doorbell - and sends the
/// new entry on to the other machines with members in the group.
fn delivering(
    shared: &Shared,
    panes: &Panes,
    act: impl FnOnce(&mut Messaging<Files>) -> Result<muster_msg::Posted, Refusal>,
) -> Result<Delivered, Reply> {
    let (mut posted, holding) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return Err(refused_as("", "handing_over", HANDING_OVER));
        }
        let posted = match act(&mut messages.service) {
            Ok(posted) => posted,
            Err(refusal) => return Err(refused("", &refusal)),
        };
        let holding = messages.hold(&posted.wakes, &posted.answered, panes);
        (posted, holding)
    };
    let rang = ring(shared, holding);
    let elsewhere = peer::tell(shared, &posted.tell);
    posted.reached.extend(elsewhere);
    if let Some(error) = &posted.unsaved {
        kept_nothing(&Refusal::Store { error: error.clone() });
    }
    let activities = activities_of(shared, &posted, panes);
    Ok(Delivered { posted, rang, activities })
}

/// What each agent a post reached on this machine is doing.
fn activities_of(
    shared: &Shared,
    posted: &muster_msg::Posted,
    panes: &Panes,
) -> HashMap<String, Activity> {
    let messages = shared.messages();
    posted
        .reached
        .iter()
        .filter_map(|(name, _)| {
            let participant = messages.service.participant(name)?;
            Some((name.clone(), panes.activity(participant)?))
        })
        .collect()
}

/// What a post or a resume did, as its answer and the daemon's log say it. `bytes` is a post's
/// size, never its body: the log records what happened, not what was said.
fn told(event: &'static str, bytes: Option<usize>, delivered: &Delivered) -> msg_answer::Posted {
    let Delivered { posted, rang, activities } = delivered;
    let until = |name: &String| {
        rang.deferred.iter().find(|(deferred, _)| deferred == name).map(|(_, until)| *until)
    };
    let told = |name: &String, reach: Reach| {
        if rang.failed.contains(name) {
            Reach::Gone
        } else if until(name).is_some() {
            Reach::Deferred
        } else {
            reach
        }
    };
    let reached: Vec<msg_answer::Reached> = posted
        .reached
        .iter()
        .map(|(name, reach)| msg_answer::Reached {
            name: name.clone(),
            reach: reach_of(told(name, *reach)).into(),
            activity: activity_of(activities.get(name).copied()).into(),
            until: until(name).unwrap_or(msg_answer::Until::Unspecified).into(),
        })
        .collect();
    let named = |wanted: Reach| {
        let names: Vec<&str> = posted
            .reached
            .iter()
            .filter(|(name, reach)| told(name, *reach) == wanted)
            .map(|(name, _)| name.as_str())
            .collect();
        names.join(",")
    };
    log::info(
        event,
        fields! {
            "author" => posted.author,
            "group" => posted.group,
            "seq" => posted.seq,
            "bytes" => bytes.unwrap_or_default(),
            "woke" => named(Reach::Woken),
            "deferred" => named(Reach::Deferred),
            "already_woken" => named(Reach::AlreadyWoken),
            "waiting" => named(Reach::Waiting),
            "no_agent" => named(Reach::NoAgent),
            "no_doorbell" => named(Reach::NoDoorbell),
            "unreachable" => named(Reach::Unreachable),
            "gone" => named(Reach::Gone),
            "paused" => named(Reach::Paused),
        },
    );
    msg_answer::Posted { group: posted.group.clone(), seq: posted.seq, reached }
}

/// A change to a group, as its answer and the daemon's log say it. What other machines must
/// be sent of it is added to `telling`.
fn changed(what: &str, changed: Changed, telling: &mut Vec<Tell>) -> (String, Answer) {
    telling.extend(changed.tell);
    log::info(
        "msg.group.changed",
        fields! {
            "by" => changed.by,
            "group" => changed.group,
            "what" => what,
            "seq" => changed.seq.unwrap_or_default(),
            "added" => changed.added.join(","),
            "removed" => changed.removed.join(","),
        },
    );
    let answer = Answer::Changed(msg_answer::Changed {
        group: changed.group,
        seq: changed.seq,
        added: changed.added,
        removed: changed.removed,
    });
    (changed.by, answer)
}

/// Delivers each wake, returning those that could not be.
fn wake(wakes: &[Wake]) -> Vec<&Wake> {
    let mut failed = Vec::new();
    for wake in wakes {
        let Via::Inbox(inbox) = &wake.via else { continue };
        let notice = notice_of(&wake.notice);
        match inbox::deliver(inbox, &messaging::wake_text(&notice)) {
            Ok(()) => log::info(
                "msg.woke",
                fields! {
                    "name" => wake.name,
                    "group" => notice.group,
                    "first" => notice.first,
                    "last" => notice.last,
                },
            ),
            Err(error) => {
                log::warn(
                    "msg.wake.failed",
                    fields! {
                        "name" => wake.name,
                        "group" => notice.group,
                        "error" => error,
                        "impact" => "the participant is marked gone and is not woken again \
                                     until it joins or runs a msg command from a live session",
                        "check" => "whether its Claude Code session has exited; a session \
                                    that is still running and refuses its inbox is a bug",
                    },
                );
                failed.push(wake);
            }
        }
    }
    failed
}

fn waiting(
    shared: &Shared,
    caller: &Caller,
    wait: &proto::msg_request::Wait,
    hung_up: &dyn Fn() -> bool,
    panes: &Panes,
) -> Reply {
    let (ticket, ended) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        match messages.service.wait(caller, wait.group.as_deref(), wait.due, panes) {
            Err(refusal) => return refused("", &refusal),
            Ok(muster_msg::Waited::Ready(notices)) => {
                let notices = notices.iter().map(notice_of).collect();
                return answered(String::new(), Answer::Notices(msg_answer::Notices { notices }));
            }
            Ok(muster_msg::Waited::Waiting { ticket, superseded }) => {
                if let Some(older) = superseded.and_then(|older| messages.waits.remove(&older)) {
                    let _ = older.send(WaitEnded::Superseded);
                }
                let (end, ended) = mpsc::channel();
                messages.waits.insert(ticket, end);
                log::debug("msg.waiting", fields! { "ticket" => ticket });
                (ticket, ended)
            }
        }
    };
    let deadline = wait.timeout_ms.map(|ms| Instant::now() + Duration::from_millis(u64::from(ms)));
    loop {
        match ended.recv_timeout(LOOK_UP) {
            Ok(WaitEnded::Ready(notices)) => {
                return answered(String::new(), Answer::Notices(msg_answer::Notices { notices }));
            }
            Ok(WaitEnded::Left) => {
                return refused_as(
                    "",
                    "left",
                    "the participant left, so nothing will answer this wait",
                );
            }
            Ok(WaitEnded::Superseded) => {
                return refused_as(
                    "",
                    "superseded",
                    "a newer wait for this participant took over; at most one runs",
                );
            }
            Err(RecvTimeoutError::Disconnected) => {
                return refused_as(
                    "",
                    "ended",
                    "the wait ended because the daemon is handing over; wait again",
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                let timed_out = deadline.is_some_and(|deadline| Instant::now() >= deadline);
                if timed_out || hung_up() {
                    let mut messages = shared.messages();
                    // A post may have answered this wait between the timeout and the lock, and
                    // counted it as woken: that answer is the one to give.
                    if let Ok(WaitEnded::Ready(notices)) = ended.try_recv() {
                        return answered(
                            String::new(),
                            Answer::Notices(msg_answer::Notices { notices }),
                        );
                    }
                    messages.service.cancel_wait(ticket);
                    messages.waits.remove(&ticket);
                    let why = if timed_out { "timed_out" } else { "hung_up" };
                    log::debug("msg.wait.ended", fields! { "ticket" => ticket, "why" => why });
                    return refused_as("", "timed_out", "nothing arrived before the timeout");
                }
            }
        }
    }
}

pub(crate) fn kept_nothing(refusal: &Refusal) {
    log::error(
        "msg.store.failed",
        fields! {
            "error" => words(refusal),
            "impact" => "what the service changed in memory is not on disk, so a daemon restart \
                         would lose it: who was woken, or a read cursor",
            "check" => "the message store beside the daemon's socket: whether the disk is full \
                        or the directory is writable by this user",
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Answers

fn answered(caller: String, answer: Answer) -> Reply {
    let answer = proto::MsgAnswer { caller, answer: Some(answer), ..proto::MsgAnswer::default() };
    Reply { detail: Some(Box::new(Detail::Msg(answer))), ..Reply::done() }
}

pub(super) fn refused(caller: &str, refusal: &Refusal) -> Reply {
    if let Refusal::Store { .. } = refusal {
        kept_nothing(refusal);
    }
    refused_as(caller, refusal.code(), &words(refusal))
}

pub(super) fn refused_as(caller: &str, code: &str, reason: &str) -> Reply {
    let answer = proto::MsgAnswer {
        caller: caller.to_string(),
        refusal: code.to_string(),
        ..proto::MsgAnswer::default()
    };
    Reply { detail: Some(Box::new(Detail::Msg(answer))), ..Reply::refused(reason) }
}

/// A refusal in words, naming the command that gets the caller past it.
// A table with one arm per refusal: split into helpers it would be the same length with the
// correspondence broken up.
#[allow(clippy::too_many_lines)]
pub(super) fn words(refusal: &Refusal) -> String {
    let join = |group: &str| messaging::command(JOIN, &format!("--group {group}"));
    match refusal {
        Refusal::BadName { name } => {
            format!(
                "{name:?} cannot be a name: use letters, digits, '.', '_' and '-', at most 64 \
                 of them, or {LONGEST_GROUP} for a group"
            )
        }
        Refusal::NameInUse { name, inbox } => format!(
            "{name} is the name of a session that is still running ({}); join under another \
             name",
            inbox.as_deref().unwrap_or("it has no inbox")
        ),
        Refusal::NoSuchGroup { group } => {
            format!("there is no group {group}; create it with `{}`", join(group))
        }
        Refusal::GroupNameClash { group, existing } => format!(
            "there is already a group {existing}, which differs from {group} only in case; use \
             --group {existing}, or another name"
        ),
        Refusal::PairTooLong { group: _ } => format!(
            "a post to all of these would make a group named after them, longer than a group \
             name may be; make one with `{}`, have them join it, and post with --group",
            messaging::command(JOIN, "--group <group>")
        ),
        Refusal::NoSuchParticipant { name } => {
            format!(
                "nobody here or on a machine linked to this one is called {name}; `{}` lists \
                 who is here",
                messaging::command(WHO, "")
            )
        }
        Refusal::WhichParticipant { name, candidates } => {
            format!("{name} could be {}; say which, as name@machine", candidates.join(" or "))
        }
        Refusal::Unreachable { group, machine } => format!(
            "{group} is kept on {machine}, which this machine cannot reach now: messages cross \
             machines only while a Muster window is attached to both, and its connection to \
             {machine} may have dropped. Groups kept on this machine still work"
        ),
        Refusal::Unchecked { group, machines } => format!(
            "no group here is called {group}, and {} cannot be reached now to ask whether it \
             keeps one. Join it by its full name, {group}@<machine>, once the connection is \
             back, or make a group here with `{}`",
            machines.join(" and "),
            messaging::command(GROUP, &format!("new {group}"))
        ),
        Refusal::Unanswered { group, machine } => format!(
            "{machine} took this and did not answer in time, so it may or may not have been done \
             there; `{}` says whether before you try again",
            messaging::command(LOG, &format!("--group {group}"))
        ),
        Refusal::KeptElsewhere { group, machine } => format!(
            "{group} is kept on {machine}, so its members, policy and pause are changed there, \
             with `muster msg` on {machine}; this machine only holds a copy"
        ),
        Refusal::HumanElsewhere { machine, calls_us } => format!(
            "this shell is the person at the Muster window on {machine}, whose messages are \
             kept there, and this machine has no link to {machine} now to do this there for \
             you: a window attached to both machines links them. Until it does, run this on \
             {machine}, where a group kept here is named <group>@{calls_us}; here you can post \
             in and change a group kept here that you are in, and read its log"
        ),
        Refusal::NotAParticipant { name } => format!(
            "{} not taking part, so there is nothing to leave",
            name.as_ref().map_or("this session is".to_string(), |name| format!("{name} is"))
        ),
        Refusal::NotAMember { name, group, permitted: None } => {
            format!("{name} is not in {group}; join it with `{}`", join(group))
        }
        Refusal::NotAMember { name, group, permitted: Some(permitted) } => format!(
            "{name} is not in {group}, and its policy lets only {} add members; ask one of them \
             to run `{}`. `{}` shows what is posted there",
            names(permitted),
            messaging::command(GROUP, &format!("add {group} {name}")),
            messaging::command(LOG, &format!("--group {group}"))
        ),
        Refusal::AddresseeNotInGroup { name, group } => format!(
            "{name} is not in {group}; post without --group to reach {name} directly, or ask \
             {name} to run `{}`",
            join(group)
        ),
        Refusal::AddressedSelf => "a post cannot be addressed to its own author".to_string(),
        Refusal::NoGroup => format!(
            "you are in no group, so a post with no --to has nowhere to go; name who it is for \
             with --to, or join a group with `{}`",
            messaging::command(JOIN, "--group <group>")
        ),
        Refusal::WhichGroup { candidates } => {
            format!("this post could go to {}; say which with --group", candidates.join(" or "))
        }
        Refusal::Unread { group, count } => format!(
            "{count} unread message{} in {group}; read {} with `{}`, then post again",
            if *count == 1 { "" } else { "s" },
            if *count == 1 { "it" } else { "them" },
            messaging::command(READ, &format!("--group {group}"))
        ),
        Refusal::EmptyBody => "the message is empty".to_string(),
        Refusal::BodyTooLarge { bytes } => format!(
            "the message is {bytes} bytes, over the {LARGEST_BODY} a message may be; write it \
             to a file and post the file's path"
        ),
        Refusal::NotAllowed { addressee, group, allowed } => format!(
            "{group}'s policy does not let you address {addressee}; you may address {}. Post \
             without --to to wake whom the policy rings for you",
            names(allowed)
        ),
        Refusal::NotUrgent { group, urgent } => format!(
            "{group}'s policy does not let you post urgently; only {} may. Post without \
             --urgent, and whom it is for are woken once they are idle",
            names(urgent)
        ),
        Refusal::NotPermitted { name, group, action, permitted } => format!(
            "{group}'s policy does not let {name} {}; only {} may",
            action_words(*action, group),
            names(permitted)
        ),
        Refusal::GroupExists { group } => format!(
            "there is already a group {group}; change its policy with `{}`",
            messaging::command(GROUP, &format!("set {group} --policy <file>"))
        ),
        Refusal::Store { error } => format!("the daemon could not keep this: {error}"),
    }
}

/// A policy's list of names, as a sentence reads it.
fn names(names: &[String]) -> String {
    let names: Vec<&str> =
        names.iter().map(|name| if name == "*" { "anyone" } else { name.as_str() }).collect();
    match names.as_slice() {
        [] => "nobody".to_string(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn action_words(action: Action, group: &str) -> String {
    match action {
        Action::Join => format!(
            "join it; ask one of them to add you with `{}`",
            messaging::command(GROUP, &format!("add {group} <you>"))
        ),
        Action::Leave => {
            "leave it; stay, and end your turn when you have nothing to add".to_string()
        }
        Action::Add => "add members".to_string(),
        Action::Remove => "remove members".to_string(),
        Action::SetPolicy => "change its policy".to_string(),
        Action::Pause => "pause it".to_string(),
        Action::Resume => "resume it".to_string(),
        Action::Delete => "delete it".to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Between the schema and the service

fn caller_of(caller: proto::msg_request::Caller) -> Caller {
    Caller {
        as_name: caller.as_name,
        inbox: caller.inbox.map(|inbox| Inbox { socket: inbox.socket, inode: inbox.inode }),
        pane: caller.pane,
        directory: caller.directory,
        at_ms: now_ms(),
    }
}

pub(super) fn entry_of(entry: &Entry) -> msg_answer::Entry {
    use msg_answer::entry::What as Said;
    let what = match &entry.what {
        What::Message { author, to, body, urgent } => Said::Message(msg_answer::Message {
            author: author.clone(),
            to: to.clone(),
            body: body.clone(),
            urgent: *urgent,
        }),
        What::Created { by } => Said::Created(by.clone()),
        What::Joined { who } => Said::Joined(who.clone()),
        What::Left { who } => Said::Left(who.clone()),
        What::Changed { by, change, policy } => Said::Changed(msg_answer::entry::Changed {
            by: by.clone(),
            policy: policy.as_deref().map(policy_of),
            change: match change {
                Change::SetPolicy => msg_answer::entry::Change::SetPolicy,
                Change::Paused => msg_answer::entry::Change::Paused,
                Change::Resumed => msg_answer::entry::Change::Resumed,
            }
            .into(),
        }),
    };
    msg_answer::Entry { seq: entry.seq, at_ms: entry.at_ms, what: Some(what) }
}

pub(crate) fn notice_of(notice: &muster_msg::Notice) -> msg_answer::Notice {
    msg_answer::Notice {
        group: notice.group.clone(),
        first: notice.first,
        last: notice.last,
        count: notice.count,
        to_you: notice.to_you,
        urgent: notice.urgent,
        from: notice.from.clone(),
        again: notice.again,
        // Only the windows read it, and `Messages::for_human` says it.
        member: false,
    }
}

pub(super) fn member_of(member: muster_msg::Member) -> msg_answer::Member {
    let liveness = match member.liveness {
        Liveness::Alive => msg_answer::Liveness::Alive,
        Liveness::Gone => msg_answer::Liveness::Gone,
        Liveness::Human => msg_answer::Liveness::Human,
        Liveness::Unreachable => msg_answer::Liveness::Unreachable,
    };
    msg_answer::Member {
        name: member.name,
        liveness: liveness.into(),
        groups: member.groups,
        inbox: member.inbox,
        activity: activity_of(member.activity).into(),
        pane: member.pane,
    }
}

pub(super) fn reach_of(reach: Reach) -> msg_answer::Reach {
    match reach {
        Reach::Woken => msg_answer::Reach::Woken,
        Reach::Deferred => msg_answer::Reach::Deferred,
        Reach::NoAgent => msg_answer::Reach::NoAgent,
        Reach::NoDoorbell => msg_answer::Reach::NoDoorbell,
        Reach::AlreadyWoken => msg_answer::Reach::AlreadyWoken,
        Reach::Waiting => msg_answer::Reach::Waiting,
        Reach::Gone => msg_answer::Reach::Gone,
        Reach::Paused => msg_answer::Reach::Paused,
        Reach::Unreachable => msg_answer::Reach::Unreachable,
    }
}

pub(super) fn policy_of(policy: &Policy) -> proto::msg_request::Policy {
    let map = |map: &std::collections::BTreeMap<String, Vec<String>>| {
        map.iter()
            .map(|(author, names)| {
                (author.clone(), proto::msg_request::Names { names: names.clone() })
            })
            .collect()
    };
    proto::msg_request::Policy {
        ring: map(&policy.ring),
        allow: map(&policy.allow),
        membership: policy.membership.clone(),
        paused: policy.paused,
        urgent: Some(proto::msg_request::Names { names: policy.urgent.clone() }),
    }
}

pub(super) fn policy_from(policy: proto::msg_request::Policy) -> Policy {
    let map = |map: HashMap<String, proto::msg_request::Names>| {
        map.into_iter().map(|(author, names)| (author, names.names)).collect()
    };
    Policy {
        ring: map(policy.ring),
        allow: map(policy.allow),
        membership: policy.membership,
        // A policy from before 1.3 says nothing about it, which is the default.
        urgent: policy.urgent.map_or_else(|| Policy::default().urgent, |names| names.names),
        paused: policy.paused,
    }
}

pub(super) fn activity_of(activity: Option<Activity>) -> msg_answer::Activity {
    match activity {
        None => msg_answer::Activity::Unspecified,
        Some(Activity::Working) => msg_answer::Activity::Working,
        Some(Activity::Blocked) => msg_answer::Activity::Blocked,
        Some(Activity::Idle) => msg_answer::Activity::Idle,
        Some(Activity::Waiting) => msg_answer::Activity::Waiting,
    }
}

pub(super) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Messages {
        let directory =
            std::env::temp_dir().join(format!("muster-messages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        Messages::load(&directory.join("daemon.sock"))
    }

    fn session(name: &str) -> Caller {
        Caller {
            inbox: Some(Inbox { socket: format!("/nonexistent/{name}.sock"), inode: 1 }),
            ..Caller::default()
        }
    }

    /// Refused for the machine, not for the policy, so a script can tell "update that daemon"
    /// from "you may not".
    #[test]
    fn an_urgent_post_to_a_machine_too_old_for_it_is_refused_with_its_own_code() {
        let speaking = |minor| proto::Version { major: 1, minor };
        let (code, words) = urgent_unsupported(true, speaking(2), "review", "devenv").unwrap();
        assert_eq!(code, "urgent_unsupported");
        assert!(
            words.starts_with("review is kept on devenv, whose muster-daemon speaks"),
            "{words}"
        );
        assert_eq!(urgent_unsupported(true, speaking(3), "review", "devenv"), None);
        assert_eq!(urgent_unsupported(false, speaking(2), "review", "devenv"), None);
    }

    #[test]
    fn the_human_is_spelled_as_the_service_names_it() {
        assert_eq!(messaging::HUMAN, muster_msg::HUMAN);
    }

    #[test]
    fn a_handover_refuses_changes_and_still_answers_questions() {
        let mut messages = scratch("handing-over");
        messages.handing_over(true);
        let join = Asked::Join(proto::msg_request::Join {
            name: Some("a".to_string()),
            group: Some("g".to_string()),
            pull: false,
        });
        let refused = messages.answer(&session("a"), join, &Panes::default());
        assert_eq!(refused.outcome, proto::Outcome::Refused);
        let who = messages.answer(
            &session("a"),
            Asked::Who(proto::msg_request::Who::default()),
            &Panes::default(),
        );
        assert_eq!(who.outcome, proto::Outcome::Done);
    }

    /// A handover that fails leaves the daemon serving, and a wait it ended must not take the
    /// next wake: its caller has gone, so the wake would be delivered nowhere.
    #[test]
    fn a_wait_a_failed_handover_ended_does_not_take_the_next_wake() {
        let mut messages = scratch("failed-handover");
        let (a, b) = (session("a"), session("b"));
        messages.service.join(&a, Some("a"), Some("g"), &Panes::default(), 1).unwrap();
        messages.service.join(&b, Some("b"), Some("g"), &Panes::default(), 2).unwrap();
        let Ok(muster_msg::Waited::Waiting { ticket, .. }) =
            messages.service.wait(&b, None, false, &Panes::default())
        else {
            panic!("b has nothing unread, so it waits");
        };
        let (end, _ended) = mpsc::channel();
        messages.waits.insert(ticket, end);

        messages.handing_over(true);
        messages.handing_over(false);
        let posted = messages.service.post(&a, None, &[], "after", &Panes::default(), 3).unwrap();
        assert!(posted.answered.is_empty(), "{posted:?}");
        assert_eq!(posted.wakes.iter().map(|wake| wake.name.as_str()).collect::<Vec<_>>(), ["b"]);
    }
}
