//! Links to other machines' daemons (MIP-4, section 11), over which groups span machines.
//!
//! The app holds each link: it tells this daemon, with a `msg.peer` request it keeps open,
//! where another machine's daemon is forwarded on this one, and this daemon dials it for as
//! long as that request lasts, again whenever the link drops. The daemon dialed serves the link
//! as a connection of the peer kind. After the two introduce themselves either side calls the
//! other on the one connection, since ssh forwards it in one direction only.
//!
//! A call is answered on a thread of its own: answering a post makes calls of its own - the new
//! entry sent on to other machines - whose replies the reader has to be free to take.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_daemon_proto::connection;
use muster_msg::{Away, Call, Peer, Reach, Refusal, Settle, Settled, Tell, is_machine};
use proto::peer_call::Call as Called;
use proto::peer_frame::Frame;
use proto::peer_reply::Reply as Replied;
use proto::{ConnectionKind, PeerFrame};

use super::presence::Panes;
use super::{Holding, LOOK_UP, now_ms, wire};
use crate::session::{Reply, Shared};

/// How long a call waits for its reply before the link counts as down for it.
const CALLING: Duration = Duration::from_secs(5);

/// A post's body is up to a mebibyte, over ssh.
const POSTING: Duration = Duration::from_secs(15);

/// How long either side waits for the other's introduction.
const INTRODUCING: Duration = Duration::from_secs(5);

/// The longest a dialer waits between attempts while the far socket does not answer: the
/// tunnel is being reopened, and the link should come back soon after it does.
const REDIAL_AT_MOST: Duration = Duration::from_secs(5);

/// One connection to another machine's daemon.
#[derive(Debug)]
pub(crate) struct Link {
    pub(crate) peer: Peer,
    writer: Mutex<UnixStream>,
    pending: Mutex<HashMap<u64, Sender<proto::PeerReply>>>,
    next: AtomicU64,
    closed: AtomicBool,
}

impl Link {
    fn new(peer: Peer, stream: &UnixStream) -> std::io::Result<Arc<Link>> {
        Ok(Arc::new(Link {
            peer,
            writer: Mutex::new(stream.try_clone()?),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        }))
    }

    fn send(&self, frame: Frame) -> std::io::Result<()> {
        let mut writer = poison::lock(&self.writer, "daemon.peer.writer");
        connection::send(&mut *writer, &PeerFrame { frame: Some(frame) })
    }

    fn call(&self, call: Called, patience: Duration) -> Result<Replied, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = mpsc::channel();
        poison::lock(&self.pending, "daemon.peer.pending").insert(id, reply);
        let sent = self.send(Frame::Call(proto::PeerCall { id, call: Some(call) }));
        if let Err(error) = sent {
            poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
            return Err(format!("the call could not be sent: {error}"));
        }
        match replied.recv_timeout(patience) {
            Ok(reply) => reply.reply.ok_or_else(|| "the reply was empty".to_string()),
            Err(RecvTimeoutError::Timeout) => {
                poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
                Err(format!("no reply within {patience:?}"))
            }
            Err(RecvTimeoutError::Disconnected) => Err("the link closed".to_string()),
        }
    }

    fn close(&self) {
        let writer = poison::lock(&self.writer, "daemon.peer.writer");
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }
}

/// Every link this daemon holds, by the machine at the other end.
#[derive(Debug, Default)]
pub(crate) struct Peers {
    links: Mutex<Vec<Arc<Link>>>,
}

impl Peers {
    fn links(&self) -> std::sync::MutexGuard<'_, Vec<Arc<Link>>> {
        poison::lock(&self.links, "daemon.peers")
    }

    /// A link to `machine`, when one is up. Two windows attached to one machine hold two; either
    /// will do.
    pub(crate) fn to(&self, machine: &str) -> Option<Arc<Link>> {
        self.links().iter().find(|link| link.peer.name == machine).cloned()
    }

    /// The machines a link is up to.
    pub(crate) fn machines(&self) -> Vec<String> {
        let machines: std::collections::BTreeSet<String> =
            self.links().iter().map(|link| link.peer.name.clone()).collect();
        machines.into_iter().collect()
    }

    /// Ends every link, as a daemon handing its panes over must: the one taking over links
    /// afresh, reading what it is sent from the store this one leaves.
    pub(crate) fn close_all(&self) {
        for link in self.links().iter() {
            link.close();
        }
    }
}

/// What this daemon calls itself to another: its host name, up to the first dot, with anything
/// a machine's name cannot hold replaced.
fn this_machine() -> String {
    let host = crate::effects::host_name();
    let short = host.split('.').next().unwrap_or_default();
    let name: String = short
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if is_machine(&name) { name } else { "machine".to_string() }
}

// ---------------------------------------------------------------------------------------------
// Holding a link, for the app

/// Links to the daemon the app says is at `socket`, which this machine calls `name`, for as
/// long as the app's request lasts.
pub(crate) fn hold(
    shared: &Arc<Shared>,
    name: &str,
    socket: &str,
    hung_up: &dyn Fn() -> bool,
) -> Reply {
    if !is_machine(name) {
        return super::refused_as(
            "",
            "bad_name",
            &format!("{name:?} cannot name a machine: use letters, digits, '.', '_' and '-'"),
        );
    }
    log::info("msg.peer.holding", fields! { "machine" => name, "socket" => socket });
    let mut wait = Duration::from_millis(250);
    let mut said = false;
    while !hung_up() {
        if shared.messages().handing_over {
            std::thread::sleep(LOOK_UP);
            continue;
        }
        match dial(name, socket) {
            Ok((stream, peer)) => {
                said = false;
                let since = std::time::Instant::now();
                let Some(link) = serve_in_background(shared, stream, peer) else { break };
                while !link.closed.load(Ordering::Acquire) && !hung_up() {
                    std::thread::sleep(LOOK_UP);
                }
                link.close();
                // A link that ends as soon as it is up would otherwise be redialed at once, and
                // again, for as long as whatever ends it lasts.
                wait = if since.elapsed() > REDIAL_AT_MOST {
                    Duration::from_millis(250)
                } else {
                    (wait * 2).min(REDIAL_AT_MOST)
                };
                pause(wait, hung_up);
            }
            Err(error) => {
                if said {
                    log::debug(
                        "msg.peer.unreachable",
                        fields! { "machine" => name, "error" => error },
                    );
                } else {
                    said = true;
                    log::warn(
                        "msg.peer.unreachable",
                        fields! {
                            "machine" => name,
                            "socket" => socket,
                            "error" => error,
                            "impact" => "groups kept on that machine refuse changes from this \
                                         one, and this machine's groups cannot reach members \
                                         there, until the link is up; this is tried again",
                            "check" => "whether the window's ssh connection to that machine is \
                                        up (`muster window` says), and whether its daemon is \
                                        running and as new as this one",
                        },
                    );
                }
                pause(wait, hung_up);
                wait = (wait * 2).min(REDIAL_AT_MOST);
            }
        }
    }
    log::info("msg.peer.released", fields! { "machine" => name });
    Reply::done()
}

/// Sleeps for `wait`, or until the app hangs up.
fn pause(wait: Duration, hung_up: &dyn Fn() -> bool) {
    let mut slept = Duration::ZERO;
    while slept < wait && !hung_up() {
        std::thread::sleep(LOOK_UP.min(wait));
        slept += LOOK_UP.min(wait);
    }
}

/// Dials the daemon at `socket` and introduces this one to it.
fn dial(name: &str, socket: &str) -> Result<(UnixStream, Peer), String> {
    let client = format!("muster-daemon {} peer", env!("CARGO_PKG_VERSION"));
    let (mut stream, _) = connection::connect(Path::new(socket), ConnectionKind::Peer, &client)
        .map_err(|error| error.to_string())?;
    let us = this_machine();
    let introduce = proto::Introduce { name: us.clone(), you: name.to_string() };
    connection::send(&mut stream, &PeerFrame { frame: Some(Frame::Introduce(introduce)) })
        .map_err(|error| format!("the introduction could not be sent: {error}"))?;
    let theirs = introduction(&mut stream)?;
    log::debug("msg.peer.introduced", fields! { "machine" => name, "calls_itself" => theirs.name });
    Ok((stream, Peer { name: name.to_string(), calls_us: us }))
}

fn introduction(stream: &mut UnixStream) -> Result<proto::Introduce, String> {
    let _ = stream.set_read_timeout(Some(INTRODUCING));
    let frame = connection::receive::<PeerFrame>(stream)?;
    let _ = stream.set_read_timeout(None);
    match frame.and_then(|frame| frame.frame) {
        Some(Frame::Introduce(introduce)) => Ok(introduce),
        other => Err(format!("the other daemon did not introduce itself: {other:?}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Serving a link

/// Serves a peer connection this daemon was dialed on, until it ends.
pub(crate) fn serve(mut stream: UnixStream, shared: &Arc<Shared>) {
    let theirs = match introduction(&mut stream) {
        Ok(theirs) if is_machine(&theirs.name) && is_machine(&theirs.you) => theirs,
        Ok(theirs) => {
            log::warn(
                "msg.peer.refused",
                fields! {
                    "calls_itself" => theirs.name,
                    "calls_us" => theirs.you,
                    "impact" => "no group spans this machine and that one",
                    "check" => "the names of both machines: a window's [[daemon]] id names the \
                                far one, and a host name the near one; both need letters, digits, \
                                '.', '_' or '-'",
                },
            );
            return;
        }
        Err(error) => {
            log::warn("msg.peer.refused", fields! { "error" => error, "impact" => "no link" });
            return;
        }
    };
    let us = proto::Introduce { name: this_machine(), you: theirs.name.clone() };
    if connection::send(&mut stream, &PeerFrame { frame: Some(Frame::Introduce(us)) }).is_err() {
        return;
    }
    let peer = Peer { name: theirs.name, calls_us: theirs.you };
    let Ok(link) = Link::new(peer, &stream) else { return };
    up(shared, &link, false);
    let why = read(shared, &link, stream);
    ended(shared, &link, &why);
}

/// Serves a link this daemon dialed on a thread of its own, returning it.
fn serve_in_background(shared: &Arc<Shared>, stream: UnixStream, peer: Peer) -> Option<Arc<Link>> {
    let link = Link::new(peer, &stream).ok()?;
    let (reading, serving) = (Arc::clone(&link), Arc::clone(shared));
    let started = std::thread::Builder::new().name("peer".to_string()).spawn(move || {
        let why = read(&serving, &reading, stream);
        ended(&serving, &reading, &why);
    });
    if let Err(error) = started {
        log::error(
            "msg.peer.no_thread",
            fields! {
                "error" => error,
                "impact" => "no link to that machine; groups kept there refuse changes from here",
                "check" => "whether the daemon is out of threads",
            },
        );
        return None;
    }
    up(shared, &link, true);
    Some(link)
}

fn up(shared: &Arc<Shared>, link: &Arc<Link>, dialed: bool) {
    shared.peers.links().push(Arc::clone(link));
    shared.messages().service.linked(&link.peer.name);
    log::info(
        "msg.peer.linked",
        fields! {
            "machine" => link.peer.name,
            "calls_us" => link.peer.calls_us,
            "dialed" => dialed,
        },
    );
    let (link, shared) = (Arc::clone(link), Arc::clone(shared));
    let _ = std::thread::Builder::new().name("peer-refetch".to_string()).spawn(move || {
        refetch(&shared, &link);
    });
}

fn ended(shared: &Shared, link: &Arc<Link>, why: &str) {
    link.closed.store(true, Ordering::Release);
    poison::lock(&link.pending, "daemon.peer.pending").clear();
    let still = {
        let mut links = shared.peers.links();
        links.retain(|held| !Arc::ptr_eq(held, link));
        links.iter().any(|held| held.peer.name == link.peer.name)
    };
    if !still {
        shared.messages().service.unlinked(&link.peer.name);
    }
    log::info("msg.peer.unlinked", fields! { "machine" => link.peer.name, "why" => why });
}

/// Reads frames until the link ends, handing replies to their callers and answering calls.
fn read(shared: &Arc<Shared>, link: &Arc<Link>, mut stream: UnixStream) -> String {
    loop {
        let frame = match connection::receive::<PeerFrame>(&mut stream) {
            Ok(Some(frame)) => frame,
            Ok(None) => return "the other daemon hung up".to_string(),
            Err(error) => return error,
        };
        match frame.frame {
            Some(Frame::Reply(reply)) => {
                let waiting = poison::lock(&link.pending, "daemon.peer.pending").remove(&reply.id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(reply);
                }
            }
            Some(Frame::Call(call)) => {
                let (shared, link) = (Arc::clone(shared), Arc::clone(link));
                let _ =
                    std::thread::Builder::new().name("peer-call".to_string()).spawn(move || {
                        let reply = answer(&shared, &link, call.call);
                        let reply = proto::PeerReply { id: call.id, reply: Some(reply) };
                        let _ = link.send(Frame::Reply(reply));
                    });
            }
            Some(Frame::Introduce(_)) | None => {}
        }
    }
}

/// Refetches every group this daemon replicates from the machine at the other end: a link that
/// comes up may follow a restart here, which kept no replica, or a cut, which kept some behind.
fn refetch(shared: &Shared, link: &Link) {
    let replicas = shared.messages().service.replicas_of(&link.peer.name);
    for (group, head) in replicas {
        fetch(shared, link, &group, head);
    }
}

/// Takes what `group`'s home holds after `head` into the replica, and wakes whoever it is for.
fn fetch(shared: &Shared, link: &Link, group: &str, mut head: u64) {
    for _ in 0..3 {
        let call = Call::Since { group: group.to_string(), after: head };
        let away = Away { machine: link.peer.name.clone(), call };
        let (settle, holding) = settle_with(shared, link, &away);
        super::ring(shared, holding);
        match settle.result {
            Ok(Settled::Gap(from)) => head = from,
            _ => return,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Calling across

/// Sends a call to the machine it is for, and settles the reply here. A machine with no link
/// now, or that does not reply in time, refuses it as unreachable, naming it.
pub(crate) fn call_away(shared: &Shared, away: &Away) -> (Settle, Holding) {
    let Some(link) = shared.peers.to(&away.machine) else {
        return (unreachable(away), Holding::default());
    };
    settle_with(shared, &link, away)
}

fn settle_with(shared: &Shared, link: &Link, away: &Away) -> (Settle, Holding) {
    let patience = if matches!(away.call, Call::Post { .. }) { POSTING } else { CALLING };
    let replied = match link.call(wire::call_to(&away.call), patience) {
        Ok(replied) => wire::reply_from(replied),
        Err(error) => {
            log::warn(
                "msg.peer.call_failed",
                fields! {
                    "machine" => away.machine,
                    "group" => away.call.group(),
                    "error" => error,
                    "impact" => "the request was refused as unreachable, and changed nothing \
                                 there unless the reply alone was lost",
                    "check" => "whether the link to that machine dropped (msg.peer.unlinked \
                                follows) or its daemon is stalled",
                },
            );
            None
        }
    };
    let Some(reply) = replied else { return (unreachable(away), Holding::default()) };
    let panes = Panes::of(shared);
    let mut messages = shared.messages();
    let settle = messages.service.settle(&link.peer, &away.call, reply, &panes, now_ms());
    let holding = messages.hold(&settle.applied.wakes, &settle.applied.answered, &panes);
    (settle, holding)
}

fn unreachable(away: &Away) -> Settle {
    let group = format!("{}@{}", away.call.group(), away.machine);
    let machine = away.machine.clone();
    Settle {
        result: Err(Refusal::Unreachable { group, machine }),
        applied: muster_msg::Applied::default(),
    }
}

/// Sends a group's new entries on to each machine named, and returns whom each reached there,
/// in this machine's names. A machine that cannot be told has its targets unreachable; it
/// catches up when its link returns.
pub(crate) fn tell(shared: &Shared, tells: &[Tell]) -> Vec<(String, Reach)> {
    let mut reached = Vec::new();
    for tell in tells {
        let unreached = || tell.targets.iter().map(|name| (name.clone(), Reach::Unreachable));
        let Some(link) = shared.peers.to(&tell.machine) else {
            reached.extend(unreached());
            continue;
        };
        let Ok(caught) = shared.messages().service.since(&tell.group, tell.after) else {
            continue;
        };
        match link.call(Called::Replicate(wire::caught_to(&caught)), CALLING) {
            Ok(Replied::Applied(applied)) => reached.extend(
                wire::reached_from(applied.reached)
                    .into_iter()
                    .map(|(name, reach)| (link.peer.inward(&name), reach)),
            ),
            Ok(other) => log::warn(
                "msg.peer.call_failed",
                fields! {
                    "machine" => tell.machine,
                    "group" => tell.group,
                    "error" => format!("a replicate was answered with {other:?}"),
                    "impact" => "that machine's members of the group were not told of the entry \
                                 until its link comes up again",
                    "check" => "whether both daemons are the same build; this is a bug if so",
                },
            ),
            Err(error) => {
                log::warn(
                    "msg.peer.call_failed",
                    fields! {
                        "machine" => tell.machine,
                        "group" => tell.group,
                        "error" => error,
                        "impact" => "that machine's members of the group see the entry when \
                                     its link comes up again, and are not woken for it until \
                                     then",
                        "check" => "whether the link to that machine dropped",
                    },
                );
                reached.extend(unreached());
            }
        }
    }
    reached
}

// ---------------------------------------------------------------------------------------------
// Answering a call

fn answer(shared: &Arc<Shared>, link: &Arc<Link>, call: Option<Called>) -> Replied {
    let refused = |refusal: &Refusal| Replied::Refused(wire::refusal_to(refusal));
    let Some(call) = call else {
        return refused(&Refusal::Store { error: "an empty call".to_string() });
    };
    if let Called::Replicate(caught) = call {
        return replicated(shared, link, caught);
    }
    let Some(call) = wire::call_from(call) else {
        return refused(&Refusal::Store { error: "a call this daemon cannot answer".to_string() });
    };
    let panes = Panes::of(shared);
    let (answered, holding) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            let group = format!("{}@{}", call.group(), link.peer.calls_us);
            let machine = link.peer.calls_us.clone();
            return refused(&Refusal::Unreachable { group, machine });
        }
        let answered = messages.service.answer(&link.peer, call.clone(), &panes, now_ms());
        let holding = messages.hold(&answered.wakes, &answered.answered, &panes);
        (answered, holding)
    };
    super::ring(shared, holding);
    if let Some(error) = &answered.unsaved {
        super::kept_nothing(&Refusal::Store { error: error.clone() });
    }
    let mut reply = answered.reply;
    let elsewhere = tell(shared, &answered.tell);
    if let muster_msg::Reply::Posted { seq, reached, .. } = &mut reply {
        reached.extend(elsewhere);
        if let Call::Post { group, body, .. } = &call {
            log::info(
                "msg.forwarded",
                fields! {
                    "machine" => link.peer.name,
                    "group" => group,
                    "seq" => *seq,
                    "bytes" => body.len(),
                },
            );
        }
    }
    wire::reply_to(reply)
}

/// A group's home sent new entries: take them into the replica, fetching first whatever came
/// between, and wake this machine's members for them.
fn replicated(shared: &Arc<Shared>, link: &Arc<Link>, caught: proto::Caught) -> Replied {
    let caught = wire::caught_from(caught);
    let group = caught.group.clone();
    let panes = Panes::of(shared);
    let (applied, holding) = {
        let mut messages = shared.messages();
        match messages.service.apply(&link.peer, caught, &panes, now_ms()) {
            Ok(applied) => {
                let holding = messages.hold(&applied.wakes, &applied.answered, &panes);
                (Some(applied), holding)
            }
            Err(head) => {
                drop(messages);
                fetch(shared, link, &group, head);
                (None, Holding::default())
            }
        }
    };
    super::ring(shared, holding);
    let reached = applied.map(|applied| wire::reached_to(&applied.reached)).unwrap_or_default();
    Replied::Applied(proto::peer_reply::Reached { reached })
}
