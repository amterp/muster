//! Messages between agents through a real daemon: the `msg` requests, the store beside the
//! socket, and the inbox a Claude session is woken through - stood in for by a socket in the
//! test that reads what it is sent, as `docs/observations/claude-code-2.1.283.md` records Claude
//! Code's own reading one line per message and saying nothing back.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use crate::support::*;
use proto::msg_answer::{self, Answer};
use proto::msg_request::{self, Request as Asked};
use proto::request::Service;
use proto::session_request;

/// A Claude session's inbox socket, as far as the daemon can tell.
struct Inbox {
    path: PathBuf,
    listener: UnixListener,
}

impl Inbox {
    fn bind(daemon: &Daemon, name: &str) -> Inbox {
        let path = daemon.root().join(format!("{name}.inbox.sock"));
        let listener = UnixListener::bind(&path).expect("the scratch root takes a socket");
        Inbox { path, listener }
    }

    fn caller(&self) -> msg_request::Caller {
        let inode = std::fs::metadata(&self.path).expect("the socket exists").ino();
        msg_request::Caller {
            inbox: Some(msg_request::Inbox { socket: self.path.display().to_string(), inode }),
            ..msg_request::Caller::default()
        }
    }

    /// Every line of the next connection the daemon makes.
    fn next_wake(&self) -> Vec<serde_json::Value> {
        let (connection, _) = self.listener.accept().expect("the daemon connects to wake");
        BufReader::new(connection)
            .lines()
            .map(|line| serde_json::from_str(&line.expect("a line")).expect("a JSON line"))
            .collect()
    }
}

fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

fn msg(caller: &msg_request::Caller, asked: Asked) -> Service {
    Service::Msg(proto::MsgRequest { caller: Some(caller.clone()), request: Some(asked) })
}

fn join(control: &mut Control, caller: &msg_request::Caller, name: &str, group: &str) {
    let asked = Asked::Join(msg_request::Join {
        name: Some(name.to_string()),
        group: Some(group.to_string()),
        pull: false,
    });
    expect(control, msg(caller, asked), proto::Outcome::Done);
}

fn post(caller: &msg_request::Caller, body: &str) -> Service {
    let asked = Asked::Post(msg_request::Post { body: body.to_string(), ..Default::default() });
    msg(caller, asked)
}

fn msg_answer(asked: &muster_harness::Asked) -> &proto::MsgAnswer {
    match &asked.answer.detail {
        Some(proto::answer::Detail::Msg(answer)) => answer,
        other => panic!("a msg request answered with {other:?}: {}", asked.answer.reason),
    }
}

fn reached(asked: &muster_harness::Asked) -> Vec<(String, msg_answer::Reach)> {
    match &msg_answer(asked).answer {
        Some(Answer::Posted(posted)) => {
            posted.reached.iter().map(|reached| (reached.name.clone(), reached.reach())).collect()
        }
        other => panic!("a post answered with {other:?}"),
    }
}

/// The messages among a read's or a log's entries, as `author: body`.
fn messages(asked: &muster_harness::Asked) -> Vec<String> {
    let Some(Answer::Entries(entries)) = &msg_answer(asked).answer else {
        panic!("expected entries, got {:?}", msg_answer(asked));
    };
    entries
        .groups
        .iter()
        .flat_map(|group| &group.entries)
        .filter_map(|entry| match &entry.what {
            Some(msg_answer::entry::What::Message(message)) => {
                Some(format!("{}: {}", message.author, message.body))
            }
            _ => None,
        })
        .collect()
}

fn read(caller: &msg_request::Caller) -> Service {
    msg(caller, Asked::Read(msg_request::Read { group: None }))
}

fn log_of(group: &str) -> Service {
    msg(
        &named("reader"),
        Asked::Log(msg_request::Log { group: group.to_string(), since: 0, follow: false }),
    )
}

#[test]
fn a_post_wakes_the_other_session_through_its_inbox_with_one_line() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (a, b) = (Inbox::bind(&daemon, "a"), Inbox::bind(&daemon, "b"));
    join(&mut control, &a.caller(), "a", "g");
    join(&mut control, &b.caller(), "b", "g");

    let posted = expect(&mut control, post(&a.caller(), "the parser is in"), proto::Outcome::Done);

    assert_eq!(reached(&posted), [("b".to_string(), msg_answer::Reach::Woken)]);
    let lines = b.next_wake();
    assert_eq!(lines.len(), 1, "one line and no auth line: {lines:?}");
    assert_eq!(lines[0]["type"], "user");
    assert_eq!(
        lines[0]["message"]["content"],
        "[muster] g: 1 new (#4), from a. Read: muster msg read --group g"
    );

    let again = expect(&mut control, post(&a.caller(), "and the lexer"), proto::Outcome::Done);
    assert_eq!(reached(&again), [("b".to_string(), msg_answer::Reach::AlreadyWoken)]);
}

#[test]
fn the_guard_refuses_a_post_on_unread_and_names_the_read_that_clears_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    expect(&mut control, post(&named("a"), "first"), proto::Outcome::Done);

    let refused = expect(&mut control, post(&named("b"), "too soon"), proto::Outcome::Refused);
    assert_eq!(msg_answer(&refused).refusal, "unread");
    assert!(
        refused.answer.reason.contains("muster msg read --group g"),
        "{}",
        refused.answer.reason
    );

    let read = expect(&mut control, read(&named("b")), proto::Outcome::Done);
    assert_eq!(messages(&read), ["a: first"]);
    expect(&mut control, post(&named("b"), "now"), proto::Outcome::Done);
}

#[test]
fn a_log_and_every_cursor_survive_a_daemon_restart() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    expect(&mut control, post(&named("a"), "before the restart"), proto::Outcome::Done);

    // Killed rather than stopped: what a post was answered for is on disk the moment it is.
    drop(control);
    daemon.kill();
    daemon.restart();
    let mut control = daemon.connect();

    let log = expect(&mut control, log_of("g"), proto::Outcome::Done);
    assert_eq!(messages(&log), ["a: before the restart"]);
    // b's cursor came back too: the message is still unread, so the guard still holds.
    let refused = expect(&mut control, post(&named("b"), "too soon"), proto::Outcome::Refused);
    assert_eq!(msg_answer(&refused).refusal, "unread");
    let read = expect(&mut control, read(&named("b")), proto::Outcome::Done);
    assert_eq!(messages(&read), ["a: before the restart"]);
    let posted = expect(&mut control, post(&named("b"), "after"), proto::Outcome::Done);
    assert_eq!(reached(&posted), [("a".to_string(), msg_answer::Reach::Waiting)]);
}

#[test]
fn a_log_survives_a_handoff_to_a_new_daemon() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    expect(&mut control, post(&named("a"), "before the handoff"), proto::Outcome::Done);
    drop(control);

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);

    let mut control = daemon.connect();
    let read = expect(&mut control, read(&named("b")), proto::Outcome::Done);
    assert_eq!(messages(&read), ["a: before the handoff"]);
}

#[test]
fn a_wait_returns_when_a_post_arrives_and_a_newer_wait_ends_the_older() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    let wait = || msg(&named("b"), Asked::Wait(msg_request::Wait::default()));

    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    let mut waiting = daemon.connect();
    waiting.send(wait());
    logging.logged_until("msg.waiting", std::time::Duration::from_secs(20));
    let posted = expect(&mut control, post(&named("a"), "go"), proto::Outcome::Done);
    assert_eq!(reached(&posted), [("b".to_string(), msg_answer::Reach::Woken)]);
    let answered = until_answer(&mut waiting);
    let Some(Answer::Notices(notices)) = &answered.answer else { panic!("{answered:?}") };
    assert_eq!(notices.notices[0].group, "g");
    assert_eq!((notices.notices[0].first, notices.notices[0].last), (4, 4));

    expect(&mut control, read(&named("b")), proto::Outcome::Done);
    let mut older = daemon.connect();
    older.send(wait());
    // Each connection is served on a thread of its own, so the newer wait is sent only once the
    // older is in the daemon: sent together, the newer can arrive first and be the one ended.
    logging.logged_times_until("msg.waiting", 2, std::time::Duration::from_secs(20));
    let mut newer = daemon.connect();
    newer.send(wait());
    let ended = until_answer(&mut older);
    assert_eq!(ended.refusal, "superseded");
}

/// A wait whose participant leaves can never be answered, so it ends rather than holding its
/// thread until the caller hangs up.
#[test]
fn leaving_ends_the_leavers_wait() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    let mut waiting = daemon.connect();
    waiting.send(msg(&named("b"), Asked::Wait(msg_request::Wait::default())));
    logging.logged_until("msg.waiting", std::time::Duration::from_secs(20));

    let leave = Asked::Leave(msg_request::Leave { group: None });
    expect(&mut control, msg(&named("b"), leave), proto::Outcome::Done);
    assert_eq!(until_answer(&mut waiting).refusal, "left");
}

/// The answer to the one request sent on `control`.
fn until_answer(control: &mut Control) -> proto::MsgAnswer {
    match control.next_message(std::time::Duration::from_secs(20)) {
        Some(proto::control_message::Message::Answer(proto::Answer {
            detail: Some(proto::answer::Detail::Msg(answer)),
            ..
        })) => answer,
        other => panic!("expected a msg answer, got {other:?}"),
    }
}

#[test]
fn a_session_whose_inbox_is_gone_is_not_woken_and_reads_as_gone() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let (a, b) = (Inbox::bind(&daemon, "a"), Inbox::bind(&daemon, "b"));
    join(&mut control, &a.caller(), "a", "g");
    join(&mut control, &b.caller(), "b", "g");
    let b_path = b.path.clone();
    drop(b);
    std::fs::remove_file(&b_path).unwrap();

    let posted = expect(&mut control, post(&a.caller(), "anyone?"), proto::Outcome::Done);
    assert_eq!(reached(&posted), [("b".to_string(), msg_answer::Reach::Gone)]);
    let who = msg(&named("reader"), Asked::Who(msg_request::Who { group: Some("g".into()) }));
    let who = expect(&mut control, who, proto::Outcome::Done);
    let Some(Answer::Members(members)) = &msg_answer(&who).answer else { panic!() };
    let liveness: Vec<_> =
        members.members.iter().map(|member| (member.name.as_str(), member.liveness())).collect();
    assert_eq!(liveness, [("a", msg_answer::Liveness::Alive), ("b", msg_answer::Liveness::Gone)]);
}

#[test]
fn messages_are_kept_for_this_user_only_and_never_written_to_the_daemon_log() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    let secret = "the body of a message, which only the store may hold";
    expect(&mut control, post(&named("a"), secret), proto::Outcome::Done);

    let store = daemon.socket_path().with_extension("msg");
    let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(store.clone()), 0o700);
    assert_eq!(mode(store.join("groups/g.log")), 0o600);
    assert_eq!(mode(store.join("state.json")), 0o600);
    assert!(std::fs::read_to_string(store.join("groups/g.log")).unwrap().contains(secret));

    let logged = logging.logged_until("msg.posted", std::time::Duration::from_secs(20));
    assert!(logged.iter().any(|line| line.line.contains("msg.posted")), "{logged:?}");
    assert!(logged.iter().all(|line| !line.line.contains(secret)), "a body reached the log");
}

const DONE: proto::Outcome = proto::Outcome::Done;

/// A caller carrying no agent identity, which is the human (MIP-4, section 3).
fn the_human() -> msg_request::Caller {
    msg_request::Caller::default()
}

fn post_to(caller: &msg_request::Caller, to: &str, body: &str) -> Service {
    let to = vec![to.to_string()];
    msg(caller, Asked::Post(msg_request::Post { to, body: body.to_string(), group: None }))
}

/// What the next event says waits for the human.
fn human_notice(window: &mut Control) -> msg_answer::Notice {
    match window.next_event().event {
        Some(proto::event::Event::HumanNotice(notice)) => notice,
        other => panic!("expected what waits for the human, got {other:?}"),
    }
}

fn attend(window: &mut Control) -> proto::Snapshot {
    let asked = expect(window, attending_request(), proto::Outcome::Done);
    let Some(proto::answer::Detail::Snapshot(snapshot)) = asked.answer.detail else {
        panic!("a subscription answered without a snapshot");
    };
    snapshot
}

/// A window attending the daemon hears of each message that wakes the human, once, and of
/// nothing said between agents; the human reading clears it (MIP-4, section 10).
#[test]
fn a_window_hears_each_message_for_the_human_once_and_no_chatter() {
    let daemon = daemon();
    let mut window = daemon.connect();
    assert_eq!(attend(&mut window).human, []);
    let mut control = daemon.connect();
    join(&mut control, &the_human(), "@human", "g");
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");

    let posted = expect(&mut control, post_to(&named("a"), "@human", "need input"), DONE);
    assert_eq!(reached(&posted), [("@human".to_string(), msg_answer::Reach::Woken)]);
    let told = human_notice(&mut window);
    assert_eq!((told.group.as_str(), told.first, told.last, told.count), ("g", 5, 5, 1));
    assert_eq!(told.from, ["a"]);

    expect(&mut control, post_to(&named("a"), "b", "the lexer is yours"), DONE);
    expect(&mut control, post_to(&named("a"), "@human", "and this"), DONE);
    // The next word is about the third message: the second, to b, said nothing to the window.
    let told = human_notice(&mut window);
    assert_eq!((told.first, told.last, told.count, told.to_you), (5, 7, 2, 2));

    expect(&mut control, read(&the_human()), DONE);
    let told = human_notice(&mut window);
    assert_eq!((told.group.as_str(), told.count), ("g", 0));
}

/// Removed from a group by a member, the human has left it, so nothing waits there any more.
#[test]
fn a_window_hears_nothing_waits_once_the_human_is_removed_from_the_group() {
    let daemon = daemon();
    let mut window = daemon.connect();
    attend(&mut window);
    let mut control = daemon.connect();
    join(&mut control, &the_human(), "@human", "g");
    join(&mut control, &named("a"), "a", "g");
    expect(&mut control, post_to(&named("a"), "@human", "need input"), DONE);
    assert_eq!(human_notice(&mut window).count, 1);

    let remove = Asked::GroupMembers(msg_request::GroupMembers {
        group: "g".to_string(),
        add: Vec::new(),
        remove: vec!["@human".to_string()],
    });
    expect(&mut control, msg(&named("a"), remove), DONE);
    let told = human_notice(&mut window);
    assert_eq!((told.group.as_str(), told.count), ("g", 0));
}

/// A group whose policy changes may wake the human for what is already there, or no longer:
/// the window is told what waits under the new one, rather than keeping the old count until
/// the next message.
#[test]
fn a_window_hears_what_waits_for_the_human_under_a_new_policy() {
    let daemon = daemon();
    let mut window = daemon.connect();
    attend(&mut window);
    let mut control = daemon.connect();
    join(&mut control, &the_human(), "@human", "g");
    join(&mut control, &named("a"), "a", "g");
    join(&mut control, &named("b"), "b", "g");
    for body in ["one", "two", "three"] {
        expect(&mut control, post(&named("a"), body), DONE);
        human_notice(&mut window);
    }
    let ringing = |names: &[&str]| {
        let names = names.iter().map(ToString::to_string).collect();
        let ring = HashMap::from([("*".to_string(), msg_request::Names { names })]);
        let policy = msg_request::Policy { ring, ..default_policy() };
        Asked::GroupSet(msg_request::GroupSet { group: "g".to_string(), policy: Some(policy) })
    };

    expect(&mut control, msg(&named("a"), ringing(&["*"])), DONE);
    let told = human_notice(&mut window);
    assert_eq!((told.group.as_str(), told.count), ("g", 0), "the agents' ring kept the banner");

    expect(&mut control, msg(&named("a"), ringing(&["*", "@human"])), DONE);
    let told = human_notice(&mut window);
    assert_eq!((told.group.as_str(), told.count), ("g", 3), "ringing the human again said nothing");
}

/// The default policy, as the schema writes it.
fn default_policy() -> msg_request::Policy {
    let names = |names: &[&str]| msg_request::Names {
        names: names.iter().map(ToString::to_string).collect(),
    };
    msg_request::Policy {
        ring: HashMap::from([("*".to_string(), names(&["*", "@human"]))]),
        allow: HashMap::from([("*".to_string(), names(&["*"]))]),
        membership: vec!["*".to_string()],
        paused: false,
    }
}

/// With no window attending, the human is not woken, and what waits is in the snapshot of the
/// window that attends next, a daemon that took over from this one included: messages to the
/// human wait unread and notify at the next launch (MIP-4, section 10).
#[test]
fn what_waits_for_the_human_reaches_the_next_window_to_attend() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &the_human(), "@human", "g");
    join(&mut control, &named("a"), "a", "g");
    // A subscriber that is not a window, `muster window --watch` say, is no one to tell.
    let mut watching = daemon.connect();
    expect(&mut watching, subscribe_request(), DONE);

    let posted = expect(&mut control, post_to(&named("a"), "@human", "need input"), DONE);
    assert_eq!(reached(&posted), [("@human".to_string(), msg_answer::Reach::Waiting)]);
    drop((control, watching));

    let answer = daemon.replace(None);
    assert_eq!(answer.outcome(), DONE, "{}", answer.reason);
    let mut window = daemon.connect();
    let waiting = attend(&mut window).human;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!((waiting[0].group.as_str(), waiting[0].count, waiting[0].to_you), ("g", 1, 1));
}

/// A follow of a log is answered once an entry lands after where it follows from, which is
/// what `muster msg log --follow` asks again and again.
#[test]
fn a_follow_of_a_log_is_answered_by_the_next_entry() {
    let daemon = daemon();
    let mut control = daemon.connect();
    join(&mut control, &named("a"), "a", "g");
    let head = 2;
    let follow = Asked::Log(msg_request::Log { group: "g".to_string(), since: head, follow: true });

    let mut logging = daemon.connect();
    let log = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(log), DONE);
    let mut following = daemon.connect();
    following.send(msg(&named("reader"), follow));
    logging.logged_until("msg.following", std::time::Duration::from_secs(20));
    join(&mut control, &named("b"), "b", "g");
    expect(&mut control, post(&named("a"), "go"), DONE);

    let answered = until_answer(&mut following);
    let Some(Answer::Entries(entries)) = &answered.answer else { panic!("{answered:?}") };
    let seqs: Vec<u64> = entries.groups[0].entries.iter().map(|entry| entry.seq).collect();
    // Woken by b's join, and answered with whatever had landed by the time it looked.
    assert_eq!(seqs.first(), Some(&3), "{entries:?}");
}
