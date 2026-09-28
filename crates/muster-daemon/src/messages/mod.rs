//! Messages between agents (MIP-4), hosted: `muster-msg`'s service over this daemon's disk and
//! Claude Code's inbox sockets, answering `msg` requests.
//!
//! It has a lock of its own rather than the session's, because a post waits on the disk and a
//! wait on another agent, and neither has anything to do with panes. Wakes are delivered with
//! that lock let go, so a session slow to take one delays only the post that woke it.

mod inbox;
mod store;

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_daemon_proto::messaging::{self, JOIN, READ, WHO};
use muster_msg::{
    Caller, Entry, Inbox, LARGEST_BODY, LONGEST_GROUP, Liveness, Messaging, Reach, Refusal, What,
};
use proto::answer::Detail;
use proto::msg_answer::{self, Answer};
use proto::msg_request::Request as Asked;

use crate::session::{HANDING_OVER, Reply, Shared};
use inbox::Sockets;
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
}

#[derive(Debug)]
enum WaitEnded {
    Ready(Vec<msg_answer::Notice>),
    Superseded,
}

impl Messages {
    /// Whatever the store beside `socket` holds, or nothing.
    pub(crate) fn load(socket: &Path) -> Messages {
        let files = Files::beside(socket);
        let found = files.load();
        Messages {
            service: Messaging::restore(files, found.saved, found.logs),
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
        }
    }

    fn answer(&mut self, caller: &Caller, asked: Asked) -> Reply {
        let changes = !matches!(asked, Asked::Who(_) | Asked::Log(_));
        if changes && self.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let result = match asked {
            Asked::Join(join) => self
                .service
                .join(caller, join.name.as_deref(), join.group.as_deref(), &Sockets, now_ms())
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
                self.service.leave(caller, leave.group.as_deref(), now_ms()).map(|left| {
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
            Asked::Who(who) => self.service.who(who.group.as_deref(), &Sockets).map(|members| {
                let members = members.into_iter().map(member_of).collect();
                (String::new(), Answer::Members(msg_answer::Members { members }))
            }),
            Asked::Read(read) => {
                self.service.read(caller, read.group.as_deref(), &Sockets).map(|read| {
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
    match request.request {
        None => Reply::unsupported(),
        Some(Asked::Post(post)) => posting(shared, &caller, &post),
        Some(Asked::Wait(wait)) => waiting(shared, &caller, &wait, hung_up),
        Some(asked) => shared.messages().answer(&caller, asked),
    }
}

fn posting(shared: &Shared, caller: &Caller, post: &proto::msg_request::Post) -> Reply {
    let posted = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        let posted = messages.service.post(
            caller,
            post.group.as_deref(),
            &post.to,
            &post.body,
            &Sockets,
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
        posted
    };

    let failed = wake(&posted.wakes);
    if !failed.is_empty() {
        let mut messages = shared.messages();
        if !messages.handing_over {
            for wake in &failed {
                if let Err(refusal) = messages.service.delivered(wake, false) {
                    kept_nothing(&refusal);
                }
            }
        }
    }
    let failed: Vec<&str> = failed.iter().map(|wake| wake.name.as_str()).collect();
    if let Some(error) = &posted.unsaved {
        kept_nothing(&Refusal::Store { error: error.clone() });
    }

    let reached: Vec<msg_answer::Reached> = posted
        .reached
        .iter()
        .map(|(name, reach)| {
            let reach = if failed.contains(&name.as_str()) { Reach::Gone } else { *reach };
            msg_answer::Reached { name: name.clone(), reach: reach_of(reach).into() }
        })
        .collect();
    let named = |wanted: Reach| {
        let names: Vec<&str> = posted
            .reached
            .iter()
            .filter(|(name, reach)| {
                (if failed.contains(&name.as_str()) { Reach::Gone } else { *reach }) == wanted
            })
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
    answered(posted.author, answer)
}

/// Delivers each wake, returning those that could not be.
fn wake(wakes: &[muster_msg::Wake]) -> Vec<&muster_msg::Wake> {
    let mut failed = Vec::new();
    for wake in wakes {
        let notice = notice_of(&wake.notice);
        match inbox::deliver(&wake.inbox, &messaging::wake_text(&notice)) {
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
) -> Reply {
    let (ticket, ended) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return refused_as("", "handing_over", HANDING_OVER);
        }
        match messages.service.wait(caller, wait.group.as_deref(), &Sockets) {
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

fn kept_nothing(refusal: &Refusal) {
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
        Refusal::NotAParticipant { name } => {
            format!("{name} is not taking part, so there is nothing to leave")
        }
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

fn notice_of(notice: &muster_msg::Notice) -> msg_answer::Notice {
    msg_answer::Notice {
        group: notice.group.clone(),
        first: notice.first,
        last: notice.last,
        count: notice.count,
        to_you: notice.to_you,
        from: notice.from.clone(),
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
    }
}

fn reach_of(reach: Reach) -> msg_answer::Reach {
    match reach {
        Reach::Woken => msg_answer::Reach::Woken,
        Reach::AlreadyWoken => msg_answer::Reach::AlreadyWoken,
        Reach::Waiting => msg_answer::Reach::Waiting,
        Reach::Gone => msg_answer::Reach::Gone,
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
    fn a_handover_refuses_changes_and_still_answers_questions() {
        let mut messages = scratch("handing-over");
        messages.handing_over(true);
        let join = Asked::Join(proto::msg_request::Join {
            name: Some("a".to_string()),
            group: Some("g".to_string()),
        });
        let refused = messages.answer(&session("a"), join);
        assert_eq!(refused.outcome, proto::Outcome::Refused);
        let who = messages.answer(&session("a"), Asked::Who(proto::msg_request::Who::default()));
        assert_eq!(who.outcome, proto::Outcome::Done);
    }

    /// A handover that fails leaves the daemon serving, and a wait it ended must not take the
    /// next wake: its caller has gone, so the wake would be delivered nowhere.
    #[test]
    fn a_wait_a_failed_handover_ended_does_not_take_the_next_wake() {
        let mut messages = scratch("failed-handover");
        let (a, b) = (session("a"), session("b"));
        messages.service.join(&a, Some("a"), Some("g"), &Sockets, 1).unwrap();
        messages.service.join(&b, Some("b"), Some("g"), &Sockets, 2).unwrap();
        let Ok(muster_msg::Waited::Waiting { ticket, .. }) =
            messages.service.wait(&b, None, &Sockets)
        else {
            panic!("b has nothing unread, so it waits");
        };
        let (end, _ended) = mpsc::channel();
        messages.waits.insert(ticket, end);

        messages.handing_over(true);
        messages.handing_over(false);
        let posted = messages.service.post(&a, None, &[], "after", &Sockets, 3).unwrap();
        assert!(posted.answered.is_empty(), "{posted:?}");
        assert_eq!(posted.wakes.iter().map(|wake| wake.name.as_str()).collect::<Vec<_>>(), ["b"]);
    }
}
