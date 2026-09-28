//! Messages between agents (MIP-4), hosted: `muster-msg`'s service over this daemon's disk and
//! Claude Code's inbox sockets, answering `msg` requests.
//!
//! It has a lock of its own rather than the session's, because a post waits on the disk and a
//! wait on another agent, and neither has anything to do with panes. Wakes are delivered with
//! that lock let go, so a session slow to take one delays only the post that woke it.

mod doorbell;
mod inbox;
mod presence;
mod prompt;
mod store;

pub(crate) use doorbell::Doorbell;

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_daemon_proto::messaging::{self, GROUP, JOIN, READ, WHO};
use muster_msg::{
    Action, Activity, Caller, Change, Changed, Entry, Inbox, LARGEST_BODY, LONGEST_GROUP, Liveness,
    Messaging, Policy, Presence, Reach, Refusal, Via, Wake, What,
};
use proto::answer::Detail;
use proto::msg_answer::{self, Answer};
use proto::msg_request::Request as Asked;

use crate::session::{HANDING_OVER, Reply, Shared};
use doorbell::Now;
use presence::Panes;
use store::Files;

/// How often a wait looks up from its channel to see whether its caller hung up.
const LOOK_UP: Duration = Duration::from_millis(500);

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
    /// What waits for the human, changed by the request being handled, for the windows to be
    /// told once this lock is let go.
    told: Vec<msg_answer::Notice>,
    /// How many times the human's notices have been handed out, which orders them.
    tellings: u64,
    /// Follows of a log waiting for its next entry, woken by anything that may have appended.
    follows: Vec<Sender<()>>,
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
        Messages {
            // A wake the daemon before this one deferred and never rang is lost with it, and one
            // it rang cannot be told from one it did not, so each is rung again once its pane
            // allows: at worst a wake too many.
            pending: service.outstanding(),
            rung: Vec::new(),
            service,
            handing_over: false,
            waits: HashMap::new(),
            told: Vec::new(),
            tellings: 0,
            follows: Vec::new(),
        }
    }

    /// What waits for the human, in each group with anything waiting.
    pub(crate) fn human(&self) -> Vec<msg_answer::Notice> {
        self.service.human_notices().iter().map(notice_of).collect()
    }

    /// What the request just handled changed for the human, in the order the service decided
    /// it, for [`tell_human`] once this lock is let go.
    fn take_told(&mut self) -> Told {
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
            // Every wake still unread is rung again, so a ring from before is not also pressed.
            self.rung.clear();
            self.pending = self.service.outstanding();
        }
    }

    fn answer(&mut self, caller: &Caller, asked: Asked, panes: &Panes) -> Reply {
        let changes = !matches!(asked, Asked::Who(_) | Asked::Log(_) | Asked::Groups(_));
        if changes && self.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let result = match asked {
            Asked::Join(join) => self
                .service
                .join(caller, join.name.as_deref(), join.group.as_deref(), panes, now_ms())
                .and_then(|joined| {
                    if join.pull {
                        self.service.pulls(&joined.name)?;
                    }
                    Ok(joined)
                })
                .map(|joined| {
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
                    });
                    (joined.name, answer)
                }),
            Asked::Leave(leave) => {
                let waits = &mut self.waits;
                let told = &mut self.told;
                self.service.leave(caller, leave.group.as_deref(), panes, now_ms()).map(|left| {
                    if let Some(wait) = left.ended.and_then(|ticket| waits.remove(&ticket)) {
                        let _ = wait.send(WaitEnded::Left);
                    }
                    if left.name == muster_msg::HUMAN {
                        told.extend(left.groups.iter().map(|group| nothing_waits(group)));
                    }
                    log::info(
                        "msg.left",
                        fields! {
                            "name" => left.name,
                            "groups" => left.groups.join(","),
                            "stopped" => left.stopped,
                        },
                    );
                    let answer = Answer::Left(msg_answer::Left {
                        groups: left.groups,
                        stopped: left.stopped,
                    });
                    (left.name, answer)
                })
            }
            Asked::Who(who) => self.service.who(who.group.as_deref(), panes).map(|members| {
                let members = members.into_iter().map(member_of).collect();
                (String::new(), Answer::Members(msg_answer::Members { members }))
            }),
            Asked::Read(read) => {
                let told = &mut self.told;
                self.service.read(caller, read.group.as_deref(), panes).map(|read| {
                    if read.name == muster_msg::HUMAN {
                        told.extend(read.groups.iter().map(|(group, _)| nothing_waits(group)));
                    }
                    let groups = read
                        .groups
                        .into_iter()
                        .map(|(group, entries)| msg_answer::GroupEntries {
                            group,
                            entries: entries.iter().map(entry_of).collect(),
                        })
                        .collect();
                    (read.name, Answer::Entries(msg_answer::Entries { groups }))
                })
            }
            Asked::Log(asked) => self.service.log(&asked.group, asked.since).map(|entries| {
                let group = msg_answer::GroupEntries {
                    group: asked.group,
                    entries: entries.iter().map(entry_of).collect(),
                };
                (String::new(), Answer::Entries(msg_answer::Entries { groups: vec![group] }))
            }),
            Asked::Groups(_)
            | Asked::GroupNew(_)
            | Asked::GroupSet(_)
            | Asked::GroupMembers(_)
            | Asked::Pause(_) => self.group(caller, asked, panes),
            Asked::Post(_) | Asked::Wait(_) | Asked::Resume(_) => {
                unreachable!("posts, waits, resumes and follows are handled apart")
            }
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
                    .map(|made| changed("made", made))
            }
            Asked::GroupSet(set) => self
                .service
                .group_set(
                    caller,
                    &set.group,
                    policy_from(set.policy.unwrap_or_default()),
                    panes,
                    now_ms(),
                )
                .map(|set| changed("set_policy", set)),
            Asked::GroupMembers(members) => {
                let waits = &mut self.waits;
                let told = &mut self.told;
                self.service
                    .group_members(
                        caller,
                        &members.group,
                        &members.add,
                        &members.remove,
                        panes,
                        now_ms(),
                    )
                    .map(|members| {
                        for ticket in &members.ended {
                            if let Some(wait) = waits.remove(ticket) {
                                let _ = wait.send(WaitEnded::Left);
                            }
                        }
                        // Removed is the human leaving, as far as the windows are concerned.
                        if members.removed.iter().any(|name| name == muster_msg::HUMAN) {
                            told.push(nothing_waits(&members.group));
                        }
                        changed("members", members)
                    })
            }
            Asked::Pause(pause) => self
                .service
                .pause(caller, &pause.group, panes, now_ms())
                .map(|paused| changed("paused", paused)),
            _ => unreachable!("only group requests are answered here"),
        }
    }
}

/// Answers a `msg` request. `hung_up` says whether the caller has gone, for a wait that would
/// otherwise outlive it.
pub(crate) fn handle(
    shared: &Shared,
    request: proto::MsgRequest,
    hung_up: &dyn Fn() -> bool,
) -> Reply {
    let caller = caller_of(request.caller.unwrap_or_default());
    let Some(asked) = request.request else { return Reply::unsupported() };
    let panes = Panes::of(shared);
    match asked {
        Asked::Post(post) => posting(shared, &caller, &post, &panes),
        Asked::Resume(resume) => resuming(shared, &caller, &resume.group, &panes),
        Asked::Wait(wait) => waiting(shared, &caller, &wait, hung_up, &panes),
        Asked::Log(log) if log.follow => following(shared, log, hung_up),
        asked => {
            let (reply, told) = {
                let mut messages = shared.messages();
                let reply = messages.answer(&caller, asked, &panes);
                messages.appended();
                (reply, messages.take_told())
            };
            tell_human(shared, told);
            reply
        }
    }
}

/// What waits for the human, as the service decided it, for the windows to be told.
struct Told {
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

fn nothing_waits(group: &str) -> msg_answer::Notice {
    msg_answer::Notice { group: group.to_string(), ..msg_answer::Notice::default() }
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

fn posting(
    shared: &Shared,
    caller: &Caller,
    post: &proto::msg_request::Post,
    panes: &Panes,
) -> Reply {
    let delivered = delivering(shared, panes, |service| {
        service.post(caller, post.group.as_deref(), &post.to, &post.body, panes, now_ms())
    });
    match delivered {
        Ok(delivered) => {
            let posted = told("msg.posted", Some(post.body.len()), &delivered);
            answered(delivered.posted.author.clone(), Answer::Posted(posted))
        }
        Err(reply) => reply,
    }
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
    /// Could not be woken.
    failed: Vec<String>,
    /// Left for the doorbell, with what each waits for.
    deferred: Vec<(String, msg_answer::Until)>,
    activities: HashMap<String, Activity>,
}

/// Runs `act` under the messaging lock, then delivers the wakes it returns with the lock let
/// go: rung, sent to an inbox, answered to a wait, or left for the doorbell.
fn delivering(
    shared: &Shared,
    panes: &Panes,
    act: impl FnOnce(&mut Messaging<Files>) -> Result<muster_msg::Posted, Refusal>,
) -> Result<Delivered, Reply> {
    // Rung or sent once the lock is let go, and deferred: left for the doorbell.
    let mut sending: Vec<Wake> = Vec::new();
    let mut ringing: Vec<(Wake, presence::Seen)> = Vec::new();
    let mut deferred: Vec<(String, msg_answer::Until)> = Vec::new();
    let (posted, activities, for_the_human) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return Err(refused_as("", "handing_over", HANDING_OVER));
        }
        let posted = act(&mut messages.service);
        let posted = match posted {
            Ok(posted) => posted,
            Err(refusal) => return Err(refused("", &refusal)),
        };
        for answered in &posted.answered {
            if let Some(wait) = messages.waits.remove(&answered.ticket) {
                let _ = wait.send(WaitEnded::Ready(vec![notice_of(&answered.notice)]));
            }
        }
        let now = Instant::now();
        for wake in &posted.wakes {
            let pane = match &wake.via {
                Via::Pane(pane) => pane,
                Via::Human => {
                    messages.told.push(notice_of(&wake.notice));
                    continue;
                }
                Via::Inbox(_) => {
                    sending.push(wake.clone());
                    continue;
                }
            };
            // A pane whose agent has not been found yet waits for it like a busy one.
            let until = match panes.get(pane).map(|seen| (doorbell::may_ring(seen, now), seen)) {
                Some((Now::Ring, seen)) => {
                    ringing.push((wake.clone(), seen.clone()));
                    continue;
                }
                Some((Now::AtIdle, _)) => msg_answer::Until::Idle,
                Some((Now::At(_), _)) => msg_answer::Until::Prompt,
                None => msg_answer::Until::Agent,
            };
            deferred.push((wake.name.clone(), until));
            messages.pending.push(wake.clone());
        }
        let activities: HashMap<String, Activity> = posted
            .reached
            .iter()
            .filter_map(|(name, _)| {
                let participant = messages.service.participant(name)?;
                Some((name.clone(), panes.activity(participant)?))
            })
            .collect();
        messages.appended();
        (posted, activities, messages.take_told())
    };
    tell_human(shared, for_the_human);
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
    let failed: Vec<String> = failed
        .iter()
        .map(|wake| wake.name.clone())
        .chain(refused_rings.into_iter().map(|(name, _)| name))
        .collect();
    if let Some(error) = &posted.unsaved {
        kept_nothing(&Refusal::Store { error: error.clone() });
    }
    Ok(Delivered { posted, failed, deferred, activities })
}

/// What a post or a resume did, as its answer and the daemon's log say it. `bytes` is a post's
/// size, never its body: the log records what happened, not what was said.
fn told(event: &'static str, bytes: Option<usize>, delivered: &Delivered) -> msg_answer::Posted {
    let Delivered { posted, failed, deferred, activities } = delivered;
    let until = |name: &String| {
        deferred.iter().find(|(deferred, _)| deferred == name).map(|(_, until)| *until)
    };
    let told = |name: &String, reach: Reach| {
        if failed.contains(name) {
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
            "gone" => named(Reach::Gone),
            "paused" => named(Reach::Paused),
        },
    );
    msg_answer::Posted { group: posted.group.clone(), seq: posted.seq, reached }
}

/// A change to a group, as its answer and the daemon's log say it.
fn changed(what: &str, changed: Changed) -> (String, Answer) {
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
    let answer = proto::MsgAnswer { caller, refusal: String::new(), answer: Some(answer) };
    Reply { detail: Some(Box::new(Detail::Msg(answer))), ..Reply::done() }
}

fn refused(caller: &str, refusal: &Refusal) -> Reply {
    if let Refusal::Store { .. } = refusal {
        kept_nothing(refusal);
    }
    refused_as(caller, refusal.code(), &words(refusal))
}

fn refused_as(caller: &str, code: &str, reason: &str) -> Reply {
    let answer =
        proto::MsgAnswer { caller: caller.to_string(), refusal: code.to_string(), answer: None };
    Reply { detail: Some(Box::new(Detail::Msg(answer))), ..Reply::refused(reason) }
}

/// A refusal in words, naming the command that gets the caller past it.
fn words(refusal: &Refusal) -> String {
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
            format!("nobody here is called {name}; `{}` lists who is", messaging::command(WHO, ""))
        }
        Refusal::NotAParticipant { name } => format!(
            "{} not taking part, so there is nothing to leave",
            name.as_ref().map_or("this session is".to_string(), |name| format!("{name} is"))
        ),
        Refusal::NotAMember { name, group } => {
            format!("{name} is not in {group}; join it with `{}`", join(group))
        }
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

fn entry_of(entry: &Entry) -> msg_answer::Entry {
    use msg_answer::entry::What as Said;
    let what = match &entry.what {
        What::Message { author, to, body } => Said::Message(msg_answer::Message {
            author: author.clone(),
            to: to.clone(),
            body: body.clone(),
        }),
        What::Created { by } => Said::Created(by.clone()),
        What::Joined { who } => Said::Joined(who.clone()),
        What::Left { who } => Said::Left(who.clone()),
        What::Changed { by, change } => Said::Changed(msg_answer::entry::Changed {
            by: by.clone(),
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
        from: notice.from.clone(),
        again: notice.again,
    }
}

fn member_of(member: muster_msg::Member) -> msg_answer::Member {
    let liveness = match member.liveness {
        Liveness::Alive => msg_answer::Liveness::Alive,
        Liveness::Gone => msg_answer::Liveness::Gone,
        Liveness::Human => msg_answer::Liveness::Human,
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

fn reach_of(reach: Reach) -> msg_answer::Reach {
    match reach {
        Reach::Woken => msg_answer::Reach::Woken,
        Reach::Deferred => msg_answer::Reach::Deferred,
        Reach::NoAgent => msg_answer::Reach::NoAgent,
        Reach::NoDoorbell => msg_answer::Reach::NoDoorbell,
        Reach::AlreadyWoken => msg_answer::Reach::AlreadyWoken,
        Reach::Waiting => msg_answer::Reach::Waiting,
        Reach::Gone => msg_answer::Reach::Gone,
        Reach::Paused => msg_answer::Reach::Paused,
    }
}

fn policy_of(policy: &Policy) -> proto::msg_request::Policy {
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
    }
}

fn policy_from(policy: proto::msg_request::Policy) -> Policy {
    let map = |map: HashMap<String, proto::msg_request::Names>| {
        map.into_iter().map(|(author, names)| (author, names.names)).collect()
    };
    Policy {
        ring: map(policy.ring),
        allow: map(policy.allow),
        membership: policy.membership,
        paused: policy.paused,
    }
}

fn activity_of(activity: Option<Activity>) -> msg_answer::Activity {
    match activity {
        None => msg_answer::Activity::Unspecified,
        Some(Activity::Working) => msg_answer::Activity::Working,
        Some(Activity::Blocked) => msg_answer::Activity::Blocked,
        Some(Activity::Idle) => msg_answer::Activity::Idle,
        Some(Activity::Waiting) => msg_answer::Activity::Waiting,
    }
}

fn now_ms() -> u64 {
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
