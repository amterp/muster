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
use muster_daemon_proto::messaging::{self, JOIN, READ, WHO};
use muster_msg::{
    Activity, Caller, Entry, Inbox, LARGEST_BODY, LONGEST_GROUP, Liveness, Messaging, Presence,
    Reach, Refusal, Via, Wake, What,
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
        }
    }

    /// Marks a handoff as under way, or as over because it failed. Taken under this lock, so a
    /// change already begun has been kept by the time the new daemon reads the store. The
    /// waits end, and their callers ask again of whichever daemon serves next.
    pub(crate) fn handing_over(&mut self, underway: bool) {
        self.handing_over = underway;
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
            self.pending = self.service.outstanding();
        }
    }

    fn answer(&mut self, caller: &Caller, asked: Asked, panes: &Panes) -> Reply {
        let changes = !matches!(asked, Asked::Who(_) | Asked::Log(_));
        if changes && self.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let result = match asked {
            Asked::Join(join) => self
                .service
                .join(caller, join.name.as_deref(), join.group.as_deref(), panes, now_ms())
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
                self.service.leave(caller, leave.group.as_deref(), panes, now_ms()).map(|left| {
                    if let Some(wait) = left.ended.and_then(|ticket| waits.remove(&ticket)) {
                        let _ = wait.send(WaitEnded::Left);
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
                self.service.read(caller, read.group.as_deref(), panes).map(|read| {
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
            Asked::Post(_) | Asked::Wait(_) => unreachable!("posts and waits are handled apart"),
        };
        match result {
            Ok((caller, answer)) => answered(caller, answer),
            Err(refusal) => refused("", &refusal),
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
        Asked::Wait(wait) => waiting(shared, &caller, &wait, hung_up, &panes),
        asked => shared.messages().answer(&caller, asked, &panes),
    }
}

fn posting(
    shared: &Shared,
    caller: &Caller,
    post: &proto::msg_request::Post,
    panes: &Panes,
) -> Reply {
    // Rung or sent once the lock is let go, and deferred: left for the doorbell.
    let mut sending: Vec<Wake> = Vec::new();
    let mut ringing: Vec<(Wake, presence::Seen)> = Vec::new();
    let mut deferred: Vec<String> = Vec::new();
    let (posted, activities) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let posted = messages.service.post(
            caller,
            post.group.as_deref(),
            &post.to,
            &post.body,
            panes,
            now_ms(),
        );
        let posted = match posted {
            Ok(posted) => posted,
            Err(refusal) => return refused("", &refusal),
        };
        for answered in &posted.answered {
            if let Some(wait) = messages.waits.remove(&answered.ticket) {
                let _ = wait.send(WaitEnded::Ready(vec![notice_of(&answered.notice)]));
            }
        }
        let now = Instant::now();
        for wake in &posted.wakes {
            let Via::Pane(pane) = &wake.via else {
                sending.push(wake.clone());
                continue;
            };
            // A pane whose agent has not been found yet waits for it like a busy one.
            match panes.get(pane).map(|seen| (doorbell::may_ring(seen, now), seen)) {
                Some((Now::Ring, seen)) => ringing.push((wake.clone(), seen.clone())),
                Some((Now::At(_) | Now::AtIdle, _)) | None => {
                    deferred.push(wake.name.clone());
                    messages.pending.push(wake.clone());
                }
            }
        }
        let activities: HashMap<String, Activity> = posted
            .reached
            .iter()
            .filter_map(|(name, _)| {
                let participant = messages.service.participant(name)?;
                Some((name.clone(), panes.activity(participant)?))
            })
            .collect();
        (posted, activities)
    };
    let names: Vec<(String, String)> =
        ringing.iter().map(|(wake, _)| (wake.name.clone(), wake.notice.group.clone())).collect();
    let came = doorbell::ring_all(shared, ringing);
    let mut refused_rings: Vec<(String, String)> = Vec::new();
    for ((name, group), came) in names.into_iter().zip(came) {
        match came {
            doorbell::Came::Rang => {}
            doorbell::Came::Waits => deferred.push(name),
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
    let failed: Vec<&str> = failed
        .iter()
        .map(|wake| wake.name.as_str())
        .chain(refused_rings.iter().map(|(name, _)| name.as_str()))
        .collect();
    if let Some(error) = &posted.unsaved {
        kept_nothing(&Refusal::Store { error: error.clone() });
    }
    told(post, &posted, &failed, &deferred, &activities)
}

/// What a post did, as its answer and the daemon's log say it: `failed` could not be woken,
/// and `deferred` are left for the doorbell.
fn told(
    post: &proto::msg_request::Post,
    posted: &muster_msg::Posted,
    failed: &[&str],
    deferred: &[String],
    activities: &HashMap<String, Activity>,
) -> Reply {
    let told = |name: &String, reach: Reach| {
        if failed.contains(&name.as_str()) {
            Reach::Gone
        } else if deferred.contains(name) {
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
    // The body's size and never the body: the log records what happened, not what was said.
    log::info(
        "msg.posted",
        fields! {
            "author" => posted.author,
            "group" => posted.group,
            "seq" => posted.seq,
            "bytes" => post.body.len(),
            "woke" => named(Reach::Woken),
            "deferred" => named(Reach::Deferred),
            "already_woken" => named(Reach::AlreadyWoken),
            "waiting" => named(Reach::Waiting),
            "gone" => named(Reach::Gone),
        },
    );
    let answer = Answer::Posted(msg_answer::Posted {
        group: posted.group.clone(),
        seq: posted.seq,
        reached,
    });
    answered(posted.author.clone(), answer)
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
        match messages.service.wait(caller, wait.group.as_deref(), panes) {
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
        Refusal::Store { error } => format!("the daemon could not keep this: {error}"),
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
            messages.service.wait(&b, None, &Panes::default())
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
