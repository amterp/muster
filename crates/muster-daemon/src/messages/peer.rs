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

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
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

/// How long a request carried to the human's home waits for its answer, but for a wait, which
/// waits for as long as its caller does. Longer than a post's, since answering it may make calls
/// of its own, to this machine among them.
const CARRYING: Duration = Duration::from_mins(1);

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
    /// Calls from the other end that it no longer wants answered: carried waits whose callers
    /// hung up.
    cancelled: Mutex<HashSet<u64>>,
}

impl Link {
    fn new(peer: Peer, stream: &UnixStream) -> std::io::Result<Arc<Link>> {
        Ok(Arc::new(Link {
            peer,
            writer: Mutex::new(stream.try_clone()?),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            cancelled: Mutex::new(HashSet::new()),
        }))
    }

    fn send(&self, frame: Frame) -> std::io::Result<()> {
        let mut writer = poison::lock(&self.writer, "daemon.peer.writer");
        connection::send(&mut *writer, &PeerFrame { frame: Some(frame) })
    }

    fn call(&self, call: Called, patience: Duration) -> Result<Replied, Failed> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = mpsc::channel();
        poison::lock(&self.pending, "daemon.peer.pending").insert(id, reply);
        let sent = self.send(Frame::Call(proto::PeerCall { id, call: Some(call) }));
        if let Err(error) = sent {
            poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
            return Err(Failed::Unsent(format!("the call could not be sent: {error}")));
        }
        match replied.recv_timeout(patience) {
            Ok(reply) => {
                reply.reply.ok_or_else(|| Failed::Unanswered("the reply was empty".to_string()))
            }
            Err(RecvTimeoutError::Timeout) => {
                poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
                Err(Failed::Unanswered(format!("no reply within {patience:?}")))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(Failed::Unanswered("the link closed after the call was sent".to_string()))
            }
        }
    }

    /// [`Self::call`] for a carried request: waits for as long as `patience` says, or with none
    /// until the reply, and ends early when `hung_up` says the caller has gone, telling the
    /// other end to stop.
    fn call_until(
        &self,
        call: Called,
        patience: Option<Duration>,
        hung_up: &dyn Fn() -> bool,
    ) -> Result<Replied, Failed> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, replied) = mpsc::channel();
        poison::lock(&self.pending, "daemon.peer.pending").insert(id, reply);
        let sent = self.send(Frame::Call(proto::PeerCall { id, call: Some(call) }));
        if let Err(error) = sent {
            poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
            return Err(Failed::Unsent(format!("the call could not be sent: {error}")));
        }
        let deadline = patience.map(|patience| std::time::Instant::now() + patience);
        loop {
            match replied.recv_timeout(LOOK_UP) {
                Ok(reply) => {
                    return reply
                        .reply
                        .ok_or_else(|| Failed::Unanswered("the reply was empty".to_string()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Failed::Unanswered(
                        "the link closed after the call was sent".to_string(),
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let late = deadline.is_some_and(|at| std::time::Instant::now() >= at);
                    if !late && !hung_up() {
                        continue;
                    }
                    poison::lock(&self.pending, "daemon.peer.pending").remove(&id);
                    let _ = self.send(Frame::Cancel(proto::Cancel { id }));
                    let why = if late {
                        format!("no reply within {patience:?}")
                    } else {
                        "its caller hung up".to_string()
                    };
                    return Err(Failed::Unanswered(why));
                }
            }
        }
    }

    fn close(&self) {
        let writer = poison::lock(&self.writer, "daemon.peer.writer");
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }
}

/// Why a call has no reply.
#[derive(Debug)]
enum Failed {
    /// It never reached the other machine, which did nothing.
    Unsent(String),
    /// It was sent, so the other machine may have acted on it and only the reply is lost.
    Unanswered(String),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Unsent(why) | Failed::Unanswered(why) => formatter.write_str(why),
        }
    }
}

/// Every link this daemon holds, by the machine at the other end.
#[derive(Debug)]
pub(crate) struct Peers {
    links: Mutex<Vec<Arc<Link>>>,
    /// Where the name this daemon calls itself to another machine is kept.
    directory: PathBuf,
    /// That name, once a link has needed it: choosing it may ask macOS, which a daemon that
    /// never links should not wait on before it answers anything.
    us: OnceLock<String>,
}

impl Peers {
    /// No links yet, for a daemon whose messages are kept at `socket`.
    pub(crate) fn beside(socket: &Path) -> Peers {
        let directory = super::store::Files::beside(socket).directory().to_path_buf();
        Peers { links: Mutex::new(Vec::new()), directory, us: OnceLock::new() }
    }

    /// What this daemon calls itself to another machine.
    fn us(&self) -> &str {
        self.us.get_or_init(|| this_machine(&self.directory))
    }

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

/// What this daemon calls itself to another machine, which writes this machine's members into
/// its logs by it for good: chosen once, and kept in `directory` from then on.
fn this_machine(directory: &Path) -> String {
    kept_name(directory, crate::effects::machine_name)
}

fn kept_name(directory: &Path, choose: impl FnOnce() -> String) -> String {
    let file = directory.join("machine");
    if let Ok(kept) = std::fs::read_to_string(&file)
        && is_machine(kept.trim())
    {
        return kept.trim().to_string();
    }
    let name = machine_name(&choose());
    let kept = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .and_then(|()| std::fs::write(&file, format!("{name}\n")));
    if let Err(error) = kept {
        log::warn(
            "msg.peer.name_not_kept",
            fields! {
                "name" => name,
                "file" => file.display().to_string(),
                "error" => error,
                "impact" => "this machine is named afresh at the next start, and another machine \
                             that knew it by an older name no longer wakes its members",
                "check" => "whether the daemon can write its message store's directory",
            },
        );
    }
    name
}

/// `host` up to its first dot, with anything a machine's name cannot hold replaced.
fn machine_name(host: &str) -> String {
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
        match dial(shared.peers.us(), name, socket) {
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
fn dial(us: &str, name: &str, socket: &str) -> Result<(UnixStream, Peer), String> {
    let client = format!("muster-daemon {} peer", env!("CARGO_PKG_VERSION"));
    let (mut stream, _) = connection::connect(Path::new(socket), ConnectionKind::Peer, &client)
        .map_err(|error| error.to_string())?;
    let introduce = proto::Introduce { name: us.to_string(), you: name.to_string() };
    connection::send(&mut stream, &PeerFrame { frame: Some(Frame::Introduce(introduce)) })
        .map_err(|error| format!("the introduction could not be sent: {error}"))?;
    let theirs = introduction(&mut stream)?;
    log::debug("msg.peer.introduced", fields! { "machine" => name, "calls_itself" => theirs.name });
    Ok((stream, Peer { name: name.to_string(), calls_us: us.to_string() }))
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
    let us = proto::Introduce { name: shared.peers.us().to_string(), you: theirs.name.clone() };
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
    shared.messages().service.linked(&link.peer);
    if !dialed && let Err(refusal) = shared.messages().service.dialed_by(&link.peer) {
        log::warn(
            "msg.peer.home_unsaved",
            fields! {
                "machine" => link.peer.name,
                "error" => super::words(&refusal),
                "impact" => "the person is known to be on that machine until this daemon \
                             restarts; after that a person's shell here is taken for a human of \
                             this machine's own again, until the link is up",
                "check" => "whether the disk holding the message store is full or read-only",
            },
        );
    }
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
                        let reply = answer(&shared, &link, call.id, call.call);
                        let reply = proto::PeerReply { id: call.id, reply: Some(reply) };
                        let _ = link.send(Frame::Reply(reply));
                    });
            }
            Some(Frame::Cancel(cancel)) => {
                poison::lock(&link.cancelled, "daemon.peer.cancelled").insert(cancel.id);
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
        let _ = fetch(shared, link, &group, head);
    }
}

/// Takes what `group`'s home holds after `head` into the replica, and wakes whoever it is for,
/// saying whom it reached.
fn fetch(shared: &Shared, link: &Link, group: &str, mut head: u64) -> Vec<(String, Reach)> {
    let mut reached = Vec::new();
    let mut gaps = 0;
    loop {
        let call = Call::Since { group: group.to_string(), after: head };
        let away = Away { machine: link.peer.name.clone(), call };
        let (settle, holding) = settle_with(shared, link, &away);
        super::ring(shared, holding);
        reached.extend(settle.applied.reached);
        match (settle.result, settle.applied.more) {
            (Ok(Settled::Gap(from)), _) if gaps < 3 => {
                gaps += 1;
                head = from;
            }
            (Ok(Settled::Caught), Some(next)) if next > head => head = next,
            _ => break,
        }
    }
    reached
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
    let changes = matches!(away.call, Call::Join { .. } | Call::Leave { .. } | Call::Post { .. });
    let replied = match link.call(wire::call_to(&away.call), patience) {
        Ok(replied) => wire::reply_from(replied).and_then(|reply| named(reply, &away.machine)),
        Err(Failed::Unanswered(error)) if changes => {
            log::warn(
                "msg.peer.call_failed",
                fields! {
                    "machine" => away.machine,
                    "group" => away.call.group(),
                    "error" => error,
                    "impact" => "the request was refused as unanswered: that machine may have \
                                 made the change, and this one's copy of the group may lack it \
                                 until it is fetched",
                    "check" => "whether the link to that machine dropped (msg.peer.unlinked \
                                follows) or its daemon is stalled",
                },
            );
            return (unanswered(shared, link, away), Holding::default());
        }
        Err(error) => {
            log::warn(
                "msg.peer.call_failed",
                fields! {
                    "machine" => away.machine,
                    "group" => away.call.group(),
                    "error" => error,
                    "impact" => "the request was refused as unreachable, and changed nothing there",
                    "check" => "whether the link to that machine dropped (msg.peer.unlinked \
                                follows) or its daemon is stalled",
                },
            );
            None
        }
    };
    let Some(reply) = replied else { return (unreachable(away), Holding::default()) };
    let panes = Panes::of(shared);
    let (settle, holding) = {
        let mut messages = shared.messages();
        let settle = messages.service.settle(&link.peer, &away.call, reply, &panes, now_ms());
        messages.let_go(&settle.applied.ended);
        let holding = messages.hold(&settle.applied.wakes, &settle.applied.answered, &panes);
        (settle, holding)
    };
    // A reply catches the replica up a page at most; the rest is fetched as a refetch is.
    // `fetch` asks `Since` itself and pages on, so only other calls start it here.
    if let Some(next) = settle.applied.more
        && !matches!(away.call, Call::Since { .. })
    {
        let _ = fetch(shared, link, away.call.group(), next);
    }
    (settle, holding)
}

/// The reply, unless it names what no participant or group here could be called, which is
/// refused whole rather than turned into this machine's names.
fn named(reply: muster_msg::Reply, machine: &str) -> Option<muster_msg::Reply> {
    match reply.check() {
        Ok(()) => Some(reply),
        Err(refusal) => {
            misnamed(machine, &refusal);
            None
        }
    }
}

fn misnamed(machine: &str, refusal: &Refusal) {
    log::warn(
        "msg.peer.misnamed",
        fields! {
            "machine" => machine,
            "refusal" => format!("{refusal:?}"),
            "impact" => "what that machine sent was refused whole and changed nothing here",
            "check" => "whether that machine's daemon is Muster's own and as new as this one; \
                        an honest one never sends such a name",
        },
    );
}

/// A change sent to `away`'s machine that was never answered: the replica may lack it, so it
/// says so until it is fetched, which is tried at once in case only the one reply was lost.
fn unanswered(shared: &Shared, link: &Link, away: &Away) -> Settle {
    let group = format!("{}@{}", away.call.group(), away.machine);
    shared.messages().service.unanswered(&group);
    let head = shared.messages().service.replicas_of(&away.machine);
    let head = head.iter().find(|(base, _)| base == away.call.group()).map_or(0, |(_, at)| *at);
    fetch(shared, link, away.call.group(), head);
    let machine = away.machine.clone();
    Settle {
        result: Err(Refusal::Unanswered { group, machine }),
        applied: muster_msg::Applied::default(),
    }
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

fn answer(shared: &Arc<Shared>, link: &Arc<Link>, id: u64, call: Option<Called>) -> Replied {
    let refused = |refusal: &Refusal| Replied::Refused(wire::refusal_to(refusal));
    let Some(call) = call else {
        return refused(&Refusal::Store { error: "an empty call".to_string() });
    };
    match call {
        Called::Replicate(caught) => return replicated(shared, link, caught),
        Called::Carried(request) => return carried(shared, link, id, request),
        _ => {}
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

/// A request the person made on the other machine, done here as the human. It ends when the
/// link does, or when the other end cancels it because its caller hung up.
fn carried(shared: &Arc<Shared>, link: &Arc<Link>, id: u64, request: proto::MsgRequest) -> Replied {
    let Some(asked) = request.request else {
        let refusal = Refusal::Store { error: "an empty carried request".to_string() };
        return Replied::Refused(wire::refusal_to(&refusal));
    };
    let hung_up = || {
        link.closed.load(Ordering::Acquire)
            || poison::lock(&link.cancelled, "daemon.peer.cancelled").contains(&id)
    };
    let reply = super::carried(shared, &link.peer, asked, &hung_up);
    poison::lock(&link.cancelled, "daemon.peer.cancelled").remove(&id);
    Replied::Carried(super::carry::reply_to(reply))
}

/// Carries a request the person made here to `machine`, where the human is homed, and answers
/// it as that machine did. None when there is no link to it, so the refusal made here stands.
pub(crate) fn carry(
    shared: &Shared,
    machine: &str,
    asked: proto::msg_request::Request,
    hung_up: &dyn Fn() -> bool,
) -> Option<Reply> {
    let link = shared.peers.to(machine)?;
    let verb = super::carry::verb(&asked);
    let patience = (!matches!(asked, proto::msg_request::Request::Wait(_))).then_some(CARRYING);
    let request = proto::MsgRequest { caller: None, request: Some(asked) };
    log::info("msg.carrying", fields! { "machine" => machine, "verb" => verb });
    let refused = |code: &str, words: &str| super::refused_as("", code, words);
    Some(match link.call_until(Called::Carried(request), patience, hung_up) {
        Ok(Replied::Carried(carried)) => super::carry::reply_from(carried),
        Ok(Replied::Refused(refused_there)) => refused(&refused_there.code, &refused_there.words),
        Ok(other) => refused(
            "mismatched",
            &format!("{machine} answered a carried {verb} with {other:?}; this is a bug"),
        ),
        Err(Failed::Unsent(_)) => return None,
        Err(Failed::Unanswered(why)) => {
            log::warn(
                "msg.peer.call_failed",
                fields! {
                    "machine" => machine,
                    "verb" => verb,
                    "error" => why,
                    "impact" => "the person's request was refused as unanswered: it may have been \
                                 done on that machine all the same",
                    "check" => "whether the link to that machine dropped (msg.peer.unlinked \
                                follows) or its daemon is stalled",
                },
            );
            refused(
                "unanswered",
                &format!(
                    "this was sent to {machine}, where your messages are kept, and no answer \
                     came back ({why}): it may have been done there. `muster msg log` there \
                     says whether"
                ),
            )
        }
    })
}

/// A group's home sent new entries: take them into the replica, fetching first whatever came
/// between, and wake this machine's members for them.
fn replicated(shared: &Arc<Shared>, link: &Arc<Link>, caught: proto::Caught) -> Replied {
    let caught = wire::caught_from(caught);
    if let Err(refusal) = caught.check() {
        misnamed(&link.peer.name, &refusal);
        return Replied::Refused(wire::refusal_to(&refusal));
    }
    let group = caught.group.clone();
    let panes = Panes::of(shared);
    // A gap is fetched, and the post that was sent on is answered with whom the fetch reached:
    // its author's machine counts those as heard.
    let (reached, holding) = {
        let mut messages = shared.messages();
        match messages.service.apply(&link.peer, caught, &panes, now_ms()) {
            Ok(applied) => {
                messages.let_go(&applied.ended);
                let holding = messages.hold(&applied.wakes, &applied.answered, &panes);
                (applied.reached, holding)
            }
            Err(head) => {
                drop(messages);
                (fetch(shared, link, &group, head), Holding::default())
            }
        }
    };
    super::ring(shared, holding);
    let reached = wire::reached_to(&reached);
    Replied::Applied(proto::peer_reply::Reached { reached })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The far machine writes this one's members as `name@<this name>` into logs it keeps for
    /// good, so the name is chosen once: a host name the network changes must not change it.
    #[test]
    fn the_name_a_machine_goes_by_is_chosen_once_and_kept() {
        let directory =
            std::env::temp_dir().join(format!("muster-machine-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(kept_name(&directory, || "office-mbp.corp.example".to_string()), "office-mbp");
        assert_eq!(kept_name(&directory, || "dhcp-10-1-2-3.hotel".to_string()), "office-mbp");
        std::fs::write(directory.join("machine"), "not a name!\n").unwrap();
        assert_eq!(
            kept_name(&directory, || "home".to_string()),
            "home",
            "a bad file is chosen again"
        );
        assert_eq!(kept_name(&directory, || "later".to_string()), "home");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
