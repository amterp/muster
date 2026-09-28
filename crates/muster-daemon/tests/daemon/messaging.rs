//! Messages between agents through a real daemon: the `msg` requests, the store beside the
//! socket, and the inbox a Claude session is woken through - stood in for by a socket in the
//! test that reads what it is sent, as `docs/observations/claude-code-2.1.283.md` records Claude
//! Code's own reading one line per message and saying nothing back.

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
    msg(&named("reader"), Asked::Log(msg_request::Log { group: group.to_string(), since: 0 }))
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
