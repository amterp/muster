//! Two daemons linked the way a window links a laptop's daemon to a devenv's (MIP-4, section 11).
//! Here both run on this machine: the near one is told the far one's socket, as the app tells
//! it the local end of the ssh forward, and cutting the link is hanging up the request that
//! holds it, as a window that quits does.

use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::support::*;
use proto::msg_answer::{self, Answer};
use proto::msg_request::{self, Request as Asked};
use proto::request::Service;
use proto::session_request;

const LINKING: Duration = Duration::from_secs(20);

/// A Claude session's inbox socket, which keeps its participant alive while it is bound.
struct Inbox {
    path: PathBuf,
    _listener: UnixListener,
}

impl Inbox {
    fn bind(daemon: &Daemon, name: &str) -> Inbox {
        let path = daemon.root().join(format!("{name}.inbox.sock"));
        let listener = UnixListener::bind(&path).expect("the scratch root takes a socket");
        Inbox { path, _listener: listener }
    }

    fn caller(&self) -> msg_request::Caller {
        let inode = std::fs::metadata(&self.path).expect("the socket exists").ino();
        msg_request::Caller {
            inbox: Some(msg_request::Inbox { socket: self.path.display().to_string(), inode }),
            ..msg_request::Caller::default()
        }
    }
}

fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

fn msg(caller: &msg_request::Caller, asked: Asked) -> Service {
    Service::Msg(proto::MsgRequest { caller: Some(caller.clone()), request: Some(asked) })
}

fn msg_answer(asked: &muster_harness::Asked) -> &proto::MsgAnswer {
    match &asked.answer.detail {
        Some(proto::answer::Detail::Msg(answer)) => answer,
        other => panic!("a msg request answered with {other:?}: {}", asked.answer.reason),
    }
}

/// Joins `group` as `name`, returning the group as the daemon names it.
fn join(control: &mut Control, caller: &msg_request::Caller, name: &str, group: &str) -> String {
    let asked = Asked::Join(msg_request::Join {
        name: Some(name.to_string()),
        group: Some(group.to_string()),
        pull: false,
    });
    let joined = expect(control, msg(caller, asked), proto::Outcome::Done);
    match &msg_answer(&joined).answer {
        Some(Answer::Joined(joined)) => joined.group.clone().unwrap_or_default(),
        other => panic!("a join answered with {other:?}"),
    }
}

fn post(caller: &msg_request::Caller, group: Option<&str>, to: &[&str], body: &str) -> Service {
    let asked = Asked::Post(msg_request::Post {
        group: group.map(str::to_string),
        to: to.iter().map(|name| (*name).to_string()).collect(),
        body: body.to_string(),
    });
    msg(caller, asked)
}

fn reached(asked: &muster_harness::Asked) -> Vec<(String, msg_answer::Reach)> {
    match &msg_answer(asked).answer {
        Some(Answer::Posted(posted)) => {
            posted.reached.iter().map(|reached| (reached.name.clone(), reached.reach())).collect()
        }
        other => panic!("a post answered with {other:?}"),
    }
}

/// The messages a read or log returned, as `group author: body`, and the machine each group
/// may be behind.
fn read(control: &mut Control, caller: &msg_request::Caller) -> (Vec<String>, Vec<String>) {
    let asked = expect(
        control,
        msg(caller, Asked::Read(msg_request::Read { group: None })),
        proto::Outcome::Done,
    );
    let Some(Answer::Entries(entries)) = &msg_answer(&asked).answer else {
        panic!("expected entries, got {:?}", msg_answer(&asked));
    };
    let mut messages = Vec::new();
    for group in &entries.groups {
        for entry in &group.entries {
            if let Some(msg_answer::entry::What::Message(message)) = &entry.what {
                messages.push(format!("{} {}: {}", group.group, message.author, message.body));
            }
        }
    }
    let behind = entries.groups.iter().filter_map(|group| group.behind.clone()).collect();
    (messages, behind)
}

fn who(control: &mut Control, group: &str) -> Vec<(String, msg_answer::Liveness)> {
    let asked = Asked::Who(msg_request::Who { group: Some(group.to_string()) });
    let answered = expect(control, msg(&named("reader"), asked), proto::Outcome::Done);
    let Some(Answer::Members(members)) = &msg_answer(&answered).answer else {
        panic!("expected members, got {:?}", msg_answer(&answered));
    };
    members.members.iter().map(|member| (member.name.clone(), member.liveness())).collect()
}

/// A daemon whose log this connection follows.
fn following(daemon: &Daemon) -> Control {
    let mut logging = daemon.connect();
    let follow = session_request::Request::FollowLog(session_request::FollowLog { after: None });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    logging
}

/// Tells `near` that the machine it calls `far` has its daemon at `far`'s socket, and returns
/// the connection holding that, once the link is up.
fn link(near: &Daemon, far: &Daemon, logging: &mut Control, times: usize) -> Control {
    let mut holding = near.connect();
    let peer = msg_request::Peer {
        name: "far".to_string(),
        socket: far.socket_path().display().to_string(),
    };
    holding.send(msg(&named("app"), Asked::Peer(peer)));
    let linked = logging.logged_times_until("msg.peer.linked", times, LINKING);
    let count = linked.iter().filter(|line| line.line.contains("msg.peer.linked")).count();
    assert_eq!(count, times, "the near daemon linked to the far one within {LINKING:?}");
    holding
}

/// Hangs up the request holding the link, and waits for both daemons to say it is down.
fn cut(holding: Control, near_log: &mut Control, far_log: &mut Control, times: usize) {
    drop(holding);
    for log in [near_log, far_log] {
        let lines = log.logged_times_until("msg.peer.unlinked", times, LINKING);
        let count = lines.iter().filter(|line| line.line.contains("msg.peer.unlinked")).count();
        assert_eq!(count, times, "each daemon saw the link go down");
    }
}

/// A member on the far daemon joins a group made on the near one by its name alone, is woken
/// from a wait by a post there, and the guard holds its own post until it has read.
#[test]
fn members_on_two_linked_daemons_share_a_group_and_the_guard_holds_across() {
    let (near, far) = (daemon(), daemon());
    let (mut near_control, mut far_control) = (near.connect(), far.connect());
    let mut near_log = following(&near);
    let _holding = link(&near, &far, &mut near_log, 1);

    assert_eq!(join(&mut near_control, &named("builder"), "builder", "review"), "review");
    let there = join(&mut far_control, &named("critic"), "critic", "review");
    assert!(there.starts_with("review@"), "the far daemon holds the near one's review: {there}");

    let mut far_log = following(&far);
    let mut waiting = far.connect();
    waiting.send(msg(&named("critic"), Asked::Wait(msg_request::Wait::default())));
    far_log.logged_until("msg.waiting", LINKING);
    let posted = expect(
        &mut near_control,
        post(&named("builder"), None, &[], "rebase first"),
        proto::Outcome::Done,
    );
    assert_eq!(reached(&posted), [("critic@far".to_string(), msg_answer::Reach::Woken)]);
    let woken = match waiting.next_message(LINKING) {
        Some(proto::control_message::Message::Answer(answer)) => answer,
        other => panic!("the far wait was answered: {other:?}"),
    };
    let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
        answer: Some(Answer::Notices(notices)),
        ..
    })) = woken.detail
    else {
        panic!("a wait answered with notices: {woken:?}");
    };
    assert_eq!(notices.notices[0].group, there);

    let refused = far_control.ask(post(&named("critic"), None, &[], "pushed"));
    assert_eq!(msg_answer(&refused).refusal, "unread", "{}", refused.answer.reason);
    let (messages, _) = read(&mut far_control, &named("critic"));
    let host = there.trim_start_matches("review@");
    assert_eq!(messages, [format!("{there} builder@{host}: rebase first")]);
    expect(&mut far_control, post(&named("critic"), None, &[], "rebased"), proto::Outcome::Done);
    let (messages, _) = read(&mut near_control, &named("builder"));
    assert_eq!(messages, ["review critic@far: rebased"]);
}

/// `who` on either daemon names every member of the group, the other machine's with that
/// machine's name, and says so when that machine cannot be asked.
#[test]
fn who_names_the_members_on_both_machines() {
    let (near, far) = (daemon(), daemon());
    let (mut near_control, mut far_control) = (near.connect(), far.connect());
    let (mut near_log, mut far_log) = (following(&near), following(&far));
    let holding = link(&near, &far, &mut near_log, 1);
    let critic = Inbox::bind(&far, "critic");
    join(&mut near_control, &named("builder"), "builder", "review");
    join(&mut far_control, &critic.caller(), "critic", "review");

    let members = who(&mut near_control, "review");
    let critic_there = members.iter().find(|(name, _)| name == "critic@far");
    assert_eq!(critic_there.map(|(_, liveness)| *liveness), Some(msg_answer::Liveness::Alive));

    cut(holding, &mut near_log, &mut far_log, 1);
    let members = who(&mut near_control, "review");
    let critic_there = members.iter().find(|(name, _)| name == "critic@far");
    assert_eq!(
        critic_there.map(|(_, liveness)| *liveness),
        Some(msg_answer::Liveness::Unreachable)
    );
}

/// While the link is down a post to a group kept on the other machine is refused at once,
/// naming it; a group kept here posts as ever; and what the near daemon's group took meanwhile
/// reaches the far member once the link is back.
#[test]
fn a_cut_link_refuses_posts_across_at_once_and_local_groups_still_post() {
    let (near, far) = (daemon(), daemon());
    let (mut near_control, mut far_control) = (near.connect(), far.connect());
    let (mut near_log, mut far_log) = (following(&near), following(&far));
    let holding = link(&near, &far, &mut near_log, 1);
    join(&mut near_control, &named("builder"), "builder", "review");
    let there = join(&mut far_control, &named("critic"), "critic", "review");
    join(&mut far_control, &named("critic"), "critic", "desk");
    join(&mut far_control, &named("scout"), "scout", "desk");

    cut(holding, &mut near_log, &mut far_log, 1);
    let started = Instant::now();
    let refused = far_control.ask(post(&named("critic"), Some("review"), &[], "anyone?"));
    assert!(started.elapsed() < Duration::from_secs(2), "refused at once: {:?}", started.elapsed());
    assert_eq!(msg_answer(&refused).refusal, "unreachable", "{}", refused.answer.reason);
    let host = there.trim_start_matches("review@");
    assert!(refused.answer.reason.contains(host), "names the machine: {}", refused.answer.reason);

    let desk = expect(
        &mut far_control,
        post(&named("critic"), Some("desk"), &[], "still here"),
        proto::Outcome::Done,
    );
    assert_eq!(reached(&desk), [("scout".to_string(), msg_answer::Reach::Waiting)]);
    let kept = expect(
        &mut near_control,
        post(&named("builder"), None, &["critic"], "while you were away"),
        proto::Outcome::Done,
    );
    assert_eq!(reached(&kept), [("critic@far".to_string(), msg_answer::Reach::Unreachable)]);
    let (_, behind) = read(&mut far_control, &named("critic"));
    assert_eq!(behind, [host.to_string()], "the replica may be behind while the link is down");

    let _holding = link(&near, &far, &mut near_log, 2);
    let deadline = Instant::now() + LINKING;
    let caught_up = loop {
        let (messages, _) = read(&mut far_control, &named("critic"));
        if !messages.is_empty() || Instant::now() > deadline {
            break messages;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(caught_up, [format!("{there} builder@{host}: while you were away")]);
}

/// A daemon that restarts or hands over holds a group kept on another machine from its first
/// answer: empty, and saying it may be behind, until the link returns and the refetch brings
/// what was posted meanwhile. Replicas are not kept, and before this the group did not exist
/// until the relink.
fn a_replica_is_held_across(start_again: fn(&mut Daemon)) {
    let (near, mut far) = (daemon(), daemon());
    let (mut near_control, mut far_control) = (near.connect(), far.connect());
    let (mut near_log, mut far_log) = (following(&near), following(&far));
    let holding = link(&near, &far, &mut near_log, 1);
    join(&mut near_control, &named("builder"), "builder", "review");
    let there = join(&mut far_control, &named("critic"), "critic", "review");
    let host = there.trim_start_matches("review@").to_string();
    expect(&mut near_control, post(&named("builder"), None, &[], "before"), proto::Outcome::Done);
    assert_eq!(read(&mut far_control, &named("critic")).0.len(), 1);
    cut(holding, &mut near_log, &mut far_log, 1);
    drop((far_control, far_log));

    start_again(&mut far);
    let mut far_control = far.connect();
    expect(
        &mut near_control,
        post(&named("builder"), None, &[], "meanwhile"),
        proto::Outcome::Done,
    );
    let (messages, behind) = read(&mut far_control, &named("critic"));
    assert!(messages.is_empty(), "nothing is there before the relink: {messages:?}");
    assert_eq!(behind, [host.as_str()], "the group is held, and may be behind");

    let _holding = link(&near, &far, &mut near_log, 2);
    let deadline = Instant::now() + LINKING;
    let caught_up = loop {
        let (messages, _) = read(&mut far_control, &named("critic"));
        if !messages.is_empty() || Instant::now() > deadline {
            break messages;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(caught_up, [format!("{there} builder@{host}: meanwhile")]);
}

#[test]
fn a_replica_is_held_across_a_restart_and_catches_up_on_relink() {
    a_replica_is_held_across(|daemon| {
        daemon.kill();
        daemon.restart();
    });
}

#[test]
fn a_replica_is_held_across_a_handoff_and_catches_up_on_relink() {
    a_replica_is_held_across(|daemon| {
        let answer = daemon.replace(None);
        assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    });
}

/// Whatever dials a daemon as a peer is held to the names an honest one sends: it cannot join or
/// leave as one of this machine's own participants, and a replicate it sends for a group this
/// machine keeps, or for a name no group could have, is refused and changes nothing here.
#[test]
fn a_peer_acting_as_this_machines_own_is_refused() {
    use proto::peer_frame::Frame;
    use proto::{peer_call, peer_reply};
    let near = daemon();
    let mut near_control = near.connect();
    join(&mut near_control, &named("builder"), "builder", "review");
    let before = read(&mut near_control, &named("builder"));

    let (mut peer, _) = proto::connection::connect(
        near.socket_path(),
        proto::ConnectionKind::Peer,
        "a test posing as another machine",
    )
    .expect("the daemon takes a peer");
    let introduce = proto::Introduce { name: "far".to_string(), you: "here".to_string() };
    let frame = |frame| proto::PeerFrame { frame: Some(frame) };
    proto::connection::send(&mut peer, &frame(Frame::Introduce(introduce))).unwrap();
    let _: Option<proto::PeerFrame> = proto::connection::receive(&mut peer).unwrap();

    let caught = |group: &str| proto::Caught {
        group: group.to_string(),
        entries: Vec::new(),
        policy: Some(msg_request::Policy {
            membership: vec!["*".to_string()],
            ..msg_request::Policy::default()
        }),
        more: false,
    };
    let calls = [
        peer_call::Call::Join(peer_call::Join {
            name: "builder@here".to_string(),
            group: "review".to_string(),
            head: 0,
        }),
        peer_call::Call::Leave(peer_call::Leave {
            name: "builder@here".to_string(),
            group: "review".to_string(),
            head: 0,
        }),
        peer_call::Call::Replicate(caught("review@here")),
        peer_call::Call::Replicate(caught("x'; sh; '")),
    ];
    for (id, call) in (1..).zip(calls) {
        let asked = proto::PeerCall { id, call: Some(call.clone()) };
        proto::connection::send(&mut peer, &frame(Frame::Call(asked))).unwrap();
        let reply = loop {
            match proto::connection::receive::<proto::PeerFrame>(&mut peer).unwrap() {
                Some(proto::PeerFrame { frame: Some(Frame::Reply(reply)) }) => break reply,
                Some(_) => {}
                None => panic!("the daemon hung up on {call:?}"),
            }
        };
        let Some(peer_reply::Reply::Refused(refused)) = reply.reply else {
            panic!("{call:?} was answered {:?}", reply.reply);
        };
        assert_eq!(refused.code, "bad_name", "{call:?}: {}", refused.words);
    }
    assert_eq!(read(&mut near_control, &named("builder")), before, "nothing changed here");
    let log = expect(
        &mut near_control,
        msg(
            &named("builder"),
            Asked::Log(msg_request::Log { group: "review".to_string(), ..Default::default() }),
        ),
        proto::Outcome::Done,
    );
    let Some(Answer::Entries(entries)) = &msg_answer(&log).answer else { panic!("a log") };
    let whats: Vec<_> = entries.groups[0].entries.iter().map(|entry| entry.what.clone()).collect();
    assert!(
        whats.iter().all(|what| !matches!(what, Some(msg_answer::entry::What::Left(_)))),
        "builder is still in review: {whats:?}"
    );
}
