//! A held-open connection that keeps a mirror current.
//!
//! Thin on purpose. Everything decidable is in `events.rs` and `snapshot.rs`, judged by
//! recorded cases with no daemon in sight; what is left here is the part that genuinely
//! needs a socket - dial, notice a hang-up, back off, and rebuild rather than resume.
//!
//! Rebuild rather than resume because herdr offers no replay. A client that reconnects and
//! carries on has patched across a gap it cannot see the size of, and structural gaps leave
//! no evidence at all (`observations/herdr-0.8.0.md` section 10). Convergent application
//! makes the rebuild cheap: the replayed session upserts onto what is already there, and
//! only the genuine differences come out as changes.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::AgentState;
use muster_core::diagnostics::poison;
use muster_core::mirror::backend::PaneId;
use muster_core::mirror::{Change, Mirror};
use muster_core::names::Names;
use serde_json::{Value, json};

use crate::client::{Failure, HerdrClient};
use crate::events::EventDecoder;
use crate::snapshot::fetch_snapshot_within;

/// What the subscription tells its owner, as it happens.
///
/// A callback rather than a channel because the shell's job is to react to a change, and a
/// queue nobody drains is a mirror that renders late. Fired with the mirror's lock
/// released, so a reaction that reads the mirror does not deadlock against the thread that
/// wrote it.
///
/// Shared and `Sync` because agent state arrives on one connection per pane, so there are
/// as many threads reporting as there are panes.
pub type Report = Arc<dyn Fn(Notice) + Send + Sync>;

/// One thing worth telling the run log or the window about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The mirror was rebuilt from an authoritative snapshot. Carries what that changed,
    /// which on a first connection is the whole session and on a reconnect is usually
    /// nothing.
    Bootstrapped {
        changes: Vec<Change>,
        dropped: usize,
    },
    Changed(Change),
    /// The connection dropped. The mirror is still the best answer available and is now a
    /// guess about the present.
    Stale {
        detail: String,
    },
    Reconnected,
    /// herdr sent an event name this build does not read. One line per name, ever.
    UnknownEvent {
        kind: String,
    },
}

/// Every session-wide subscription herdr offers that Muster reads.
///
/// Dotted here and snake on the way back: a client subscribes to `pane.created` and is
/// answered with `pane_created`. The two lists have to be kept in step by hand, and the
/// failure when they are not is silence rather than an error - herdr accepts a
/// subscription and simply never sends anything, which reads as a daemon with nothing
/// happening in it.
///
/// Not `pane.agent_status_changed`, which is the one that matters most: it takes a
/// `pane_id` and no unparameterized subscription carries the same information
/// (`observations/herdr-0.8.0.md` section 11), so agent state costs a connection per pane
/// and is subscribed separately.
pub const STRUCTURE: [&str; 18] = [
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "workspace.focused",
    "tab.created",
    "tab.renamed",
    "tab.closed",
    "tab.focused",
    // A tab reordered, which nothing in Muster causes - another client, or herdr's own TUI.
    // Named here because it is the *only* thing that says so: a client subscribed to every
    // other structural kind sees nothing at all while the order changes under it, where a pane
    // move at least produced a `layout.updated` (`observations/herdr-0.8.0.md` section 21).
    "tab.moved",
    "pane.created",
    "pane.updated",
    "pane.closed",
    "pane.exited",
    "pane.focused",
    // A pane carried into another tab, which Muster itself causes whenever a row is dropped
    // on a row in a different one. Measured absent from every other subscription in this
    // list - a client that asks for all sixteen of the others sees only `layout.updated` for
    // a move (`observations/herdr-0.8.0.md` section 20) - so this is the only route by which
    // the pane's new tab is ever stated.
    "pane.moved",
    "pane.agent_detected",
    // The only live description of how a tab arranges its panes. Absent, the mirror still
    // gets a tree from every snapshot and would look correct until somebody split
    // something - which is the silence this list's doc comment is about.
    "layout.updated",
];

/// A subscription's control handle.
///
/// Dropping it ends the connection and the thread. Held rather than detached so that a
/// window closing does not leave a daemon streaming into nothing.
#[derive(Debug)]
pub struct Subscription {
    running: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
    /// Set by the thread on its way out.
    stopped: Arc<AtomicBool>,
}

impl Subscription {
    /// Starts a thread that keeps `mirror` current until this handle is dropped.
    ///
    /// Returns immediately, before the first snapshot: the window should come up saying
    /// `disconnected` rather than blocking on a daemon that may not be there. `Health`
    /// already carries that state, and the first `Bootstrapped` replaces it.
    pub fn start(
        socket_path: impl Into<String>,
        mirror: Arc<Mutex<Mirror>>,
        report: Report,
        names: Names,
    ) -> Subscription {
        let socket_path = socket_path.into();
        let running = Arc::new(AtomicBool::new(true));
        let stream = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));

        let handle = Subscription {
            running: running.clone(),
            stream: stream.clone(),
            stopped: stopped.clone(),
        };
        std::thread::Builder::new()
            .name("muster-subscription".to_string())
            .spawn(move || {
                run(&socket_path, &mirror, &report, &running, &stream, &names);
                stopped.store(true, Ordering::Release);
            })
            .expect("could not start the subscription thread");
        handle
    }

    /// Whether the thread has finished, for a test that needs to prove it does.
    ///
    /// The same arrangement as `PaneControlChannel::stopped`, for the same reason. A thread
    /// still holding a connection to a daemon nobody is watching is invisible from outside the
    /// process and unobservable through any other part of this API - and it is exactly what a
    /// handle that failed to stop one would leave behind, one per daemon, for the life of the
    /// window. Handed out as a flag rather than a join handle because joining inside `drop`
    /// would turn a thread that did not notice into a hang on quit, which is worse than the
    /// leak.
    pub fn stopped(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // Shut the socket down rather than waiting for the read to return. The thread is
        // parked in `read` with no timeout, and a flag alone would not be noticed until
        // the daemon happened to say something.
        if let Some(stream) = poison::lock(&self.stream, "subscription-stream").as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Backoff between attempts, and then every attempt after.
///
/// Short at first because the overwhelmingly common drop is a daemon restarting under a
/// developer's hands, and a window that takes eight seconds to notice it is back feels
/// broken. Capped low for the same reason - this is a socket on the same machine, or an
/// SSH tunnel to one, and there is no server to protect from a stampede.
const BACKOFF: [Duration; 4] = [
    Duration::from_millis(50),
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_secs(1),
];

fn run(
    socket_path: &str,
    mirror: &Arc<Mutex<Mirror>>,
    report: &Report,
    running: &Arc<AtomicBool>,
    shared_stream: &Arc<Mutex<Option<UnixStream>>>,
    names: &Names,
) {
    let mut attempt = 0usize;
    let mut connected_before = false;
    // Two flags rather than one, because they answer different questions and a failed
    // bootstrap is where the two part company. Whether to say "reconnected" is about the
    // socket; whether the mirror is stale or disconnected is about whether it has ever held a
    // session, and a connection that was acknowledged and then said nothing has not given it
    // one.
    let mut held_a_session = false;

    let structure: Vec<Value> = STRUCTURE.iter().map(|kind| json!({ "type": kind })).collect();
    let mut agents = AgentWatchers::new(socket_path, mirror, report, names);

    while running.load(Ordering::Relaxed) {
        match connect(socket_path, &structure, shared_stream) {
            Ok(stream) => {
                if connected_before {
                    report(Notice::Reconnected);
                }
                connected_before = true;

                // Snapshot after subscribing, never before. Between a snapshot and a
                // later subscribe there is a window in which an event fires and reaches
                // nobody, and it is invisible: the mirror would be wrong in a way no
                // counter reports. Subscribing first makes the overlap a duplicate
                // instead, which upsert already absorbs.
                let detail = match bootstrap(socket_path, mirror, report, names) {
                    Ok(()) => {
                        held_a_session = true;
                        // Reset here rather than on the dial, so that a daemon which keeps
                        // accepting and keeps failing to describe itself backs off like any
                        // other failure instead of being asked again every fifty milliseconds.
                        attempt = 0;

                        agents.follow();
                        stream_events(stream, mirror, report, running, &mut agents, names)
                    }
                    // A connection with no session behind it is worse than no connection.
                    // Every event on it describes a change to a world the mirror does not
                    // have, and nothing would ever fetch that world again: the connection is
                    // healthy, so there is no reconnect, and a reconnect is the only thing
                    // that bootstraps. Streaming it anyway is how a snapshot that took
                    // slightly too long became a window that stayed empty until the daemon
                    // restarted. So this ends the attempt, which puts it back on the backoff
                    // loop below like any other failure.
                    Err(failure) => {
                        drop(stream);
                        format!(
                            "the daemon acknowledged the subscription and then would not \
                             describe its session ({failure})"
                        )
                    }
                };

                *poison::lock(shared_stream, "subscription-stream") = None;
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                {
                    // Stale once a session has been held, disconnected before that, on the
                    // same terms as a failed dial: a mirror that never got a snapshot has no
                    // last good answer to label as aging, and calling an empty one stale would
                    // offer a session nobody has ever seen as one that merely needs refreshing.
                    let mut mirror = poison::lock(mirror, "mirror");
                    if held_a_session {
                        mirror.mark_stale(&detail);
                    } else {
                        mirror.mark_disconnected(&detail);
                    }
                }
                report(Notice::Stale { detail });
            }
            Err(detail) => {
                // Whatever the failed attempt left behind is closed, and holding it would
                // have the next `Drop` shut down a socket that is already gone.
                *poison::lock(shared_stream, "subscription-stream") = None;
                {
                    // Disconnected rather than stale on a failed dial: nothing has been
                    // reached, so there is no last good answer to label as aging.
                    let mut mirror = poison::lock(mirror, "mirror");
                    if held_a_session {
                        mirror.mark_stale(&detail);
                    } else {
                        mirror.mark_disconnected(&detail);
                    }
                }
                report(Notice::Stale { detail });
            }
        }

        let wait = BACKOFF[attempt.min(BACKOFF.len() - 1)];
        attempt += 1;
        // Slept in slices so that dropping the handle is noticed within a frame rather
        // than at the end of the current backoff.
        let mut slept = Duration::ZERO;
        while slept < wait && running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            slept += Duration::from_millis(10);
        }
    }
}

/// Dials, subscribes, and waits for the daemon to say it worked.
///
/// The write side stays open, unlike every other request this crate sends. An ordinary
/// herdr call is one line answered with one line, and half-closing is what tells the
/// daemon the request is complete - do the same here and it treats the subscription as
/// finished and hangs up immediately, which looks exactly like a daemon with nothing
/// happening in it.
///
/// The acknowledgement is read rather than fed to the decoder. It is the only place a
/// rejected subscription is visible: a name herdr does not know is refused here and
/// otherwise costs nothing but silence on that event for as long as the app runs.
///
/// `held` is filled the moment the socket exists, before a byte is written, because the
/// read below has no timeout: a daemon that accepts and then stalls parks this thread
/// forever, and the only thing that unparks it is a shutdown from the handle's `Drop`.
/// Anything published later is published too late to be shut down.
fn connect(
    socket_path: &str,
    subscriptions: &[Value],
    held: &Arc<Mutex<Option<UnixStream>>>,
) -> Result<UnixStream, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| error.to_string())?;
    // A clone that cannot be made is a connection that cannot be abandoned, so it counts as
    // a failed dial rather than a stream nobody can reach. The way this fails is running out
    // of descriptors, which is the same exhaustion an unshutdownable subscription causes.
    let shared = stream
        .try_clone()
        .map_err(|error| format!("the subscription could not be shared to be closed: {error}"))?;
    *poison::lock(held, "subscription-stream") = Some(shared);

    let request = json!({
        "id": "muster:subscribe",
        "method": "events.subscribe",
        "params": { "subscriptions": subscriptions },
    });
    let mut payload = request.to_string().into_bytes();
    payload.push(b'\n');
    stream.write_all(&payload).map_err(|error| error.to_string())?;

    let acknowledgement = read_line(&mut stream)
        .ok_or_else(|| "the daemon hung up without answering the subscription".to_string())?;
    let acknowledgement: Value = serde_json::from_slice(&acknowledgement).map_err(|_| {
        "the daemon answered the subscription with something unreadable".to_string()
    })?;
    if let Some(error) = acknowledgement.get("error") {
        return Err(format!("the daemon refused the subscription: {error}"));
    }
    Ok(stream)
}

/// Reads one newline-terminated line, for the acknowledgement only.
///
/// A byte at a time because the next byte after the newline belongs to the stream, and
/// reading it into a buffer this function owns would drop the first event of every
/// connection.
fn read_line(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 1 << 16 {
        match stream.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => return Some(line),
            Ok(1) => line.push(byte[0]),
            _ => return None,
        }
    }
    Some(line)
}

/// How long the session is worth waiting for before the attempt counts as failed.
///
/// Not the client's own default, which is 500ms because that client sits on the input path and
/// a wedged daemon must not take the keyboard with it. Nothing renders until this call answers,
/// so giving up on a daemon that is merely busy costs the whole window rather than one
/// keystroke - and giving up used to cost it permanently.
///
/// Five seconds against a snapshot measured at 0.7ms (`docs/testing.md`). The bound it has to
/// clear is a machine under load rather than a daemon under load, which is why it is nowhere
/// near the measurement: the harness allows twenty seconds for a daemon's first ping and
/// `client_connection.rs` spends ten on a call that waits for something to happen. Past five
/// the retry below is the better answer anyway, because by then the window has been told.
const SESSION_ALLOWANCE: Duration = Duration::from_secs(5);

/// Rebuilds the mirror from the daemon's own answer, so events have a world to describe.
///
/// Answers with the failure rather than swallowing it. What used to be here discarded it and
/// returned, and the caller carried straight on to streaming - see `run` for what that cost.
fn bootstrap(
    socket_path: &str,
    mirror: &Arc<Mutex<Mirror>>,
    report: &Report,
    names: &Names,
) -> Result<(), Failure> {
    let (snapshot, dropped) = fetch_snapshot_within(socket_path, names, SESSION_ALLOWANCE)?;
    let changes = poison::lock(mirror, "mirror").bootstrap(snapshot);
    report(Notice::Bootstrapped { changes, dropped });
    Ok(())
}

/// Reads until the daemon hangs up, and says why it stopped.
fn stream_events(
    mut stream: UnixStream,
    mirror: &Arc<Mutex<Mirror>>,
    report: &Report,
    running: &Arc<AtomicBool>,
    agents: &mut AgentWatchers,
    names: &Names,
) -> String {
    let mut decoder = EventDecoder::new(names.clone());
    let mut buffer = [0u8; 8192];

    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) => return "the daemon closed the connection".to_string(),
            Ok(read) => read,
            Err(error) => return error.to_string(),
        };
        if !running.load(Ordering::Relaxed) {
            return "shutting down".to_string();
        }

        let events = decoder.consume(&buffer[..read]);
        for kind in decoder.take_unknown_kinds() {
            report(Notice::UnknownEvent { kind });
        }

        // Applied under the lock, reported outside it. A report that reads the mirror -
        // which is what rendering a change means - would otherwise deadlock against the
        // thread that is still holding it.
        let changes: Vec<Change> = {
            let mut mirror = poison::lock(mirror, "mirror");
            events.into_iter().flat_map(|event| mirror.apply(event)).collect()
        };
        // After the changes are applied, so the set of panes it reads is the current one.
        // Cheap when nothing structural happened, which is most events.
        if changes
            .iter()
            .any(|change| matches!(change, Change::PaneAdded(_) | Change::PaneRemoved { .. }))
        {
            agents.follow();
        }
        for change in changes {
            report(Notice::Changed(change));
        }
    }
}

/// One held-open connection per pane, because that is what herdr charges for agent state.
///
/// `pane.agent_status_changed` takes a `pane_id` and no session-wide subscription carries
/// the same information (`observations/herdr-0.8.0.md` section 11). So an overview of N
/// panes costs N connections plus the one for structure, and the alternative - polling
/// `session.snapshot` - pays a connect per poll and picks its own staleness.
///
/// Every pane, not only the attached one. The product's reason to exist is showing all of
/// them at a glance, so watching one and adding the rest later would be measuring the
/// cheap case and shipping the expensive one. The cost of this arrangement is the number
/// that decides whether the upstream ask (`a_27N6pklkl`) is worth pressing.
struct AgentWatchers {
    socket_path: String,
    mirror: Arc<Mutex<Mirror>>,
    report: Report,
    names: Names,
    watching: BTreeMap<PaneId, Watcher>,
}

/// A watcher's off switch. Dropping it shuts the socket down, which is what unparks the
/// thread from its blocking read and ends it.
#[derive(Debug)]
struct Watcher {
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(stream) = poison::lock(&self.stream, "watcher-stream").as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Hand-written because a `Report` is a closure and closures carry no `Debug`. Finished
/// non-exhaustively for the same reason, rather than printing a placeholder for a callback
/// nobody can read anything into.
impl std::fmt::Debug for AgentWatchers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentWatchers")
            .field("socket_path", &self.socket_path)
            .field("mirror", &self.mirror)
            .field("watching", &self.watching.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl AgentWatchers {
    fn new(
        socket_path: &str,
        mirror: &Arc<Mutex<Mirror>>,
        report: &Report,
        names: &Names,
    ) -> AgentWatchers {
        AgentWatchers {
            socket_path: socket_path.to_string(),
            mirror: Arc::clone(mirror),
            report: Arc::clone(report),
            names: names.clone(),
            watching: BTreeMap::new(),
        }
    }

    /// Brings the set of watchers in line with the panes the mirror holds.
    fn follow(&mut self) {
        let mirror = poison::lock(&self.mirror, "mirror");
        let wanted: Vec<PaneId> = mirror.panes().map(|pane| pane.id.clone()).collect();
        drop(mirror);

        self.watching.retain(|pane, _| wanted.contains(pane));
        for pane in wanted {
            if !self.watching.contains_key(&pane) {
                let watcher = self.watch(&pane);
                self.watching.insert(pane, watcher);
            }
        }
    }

    fn watch(&self, pane: &PaneId) -> Watcher {
        let slot: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let socket_path = self.socket_path.clone();
        let mirror = Arc::clone(&self.mirror);
        let report = Arc::clone(&self.report);
        let names = self.names.clone();
        let held = Arc::clone(&slot);
        // The daemon's own id, because this is a request. A name that resolves to nothing is a
        // pane the mirror holds and the registry does not, which cannot happen - the mirror is
        // filled through the registry - so there is nothing here to report and no watcher to
        // start.
        let Ok(backend) = names.backend_pane(pane) else { return Watcher { stream: slot } };
        let subscription =
            vec![json!({ "type": "pane.agent_status_changed", "pane_id": backend.as_str() })];
        let pane = pane.clone();

        let _ = std::thread::Builder::new().name(format!("muster-agent-{pane}")).spawn(move || {
            // No retry loop of its own. A watcher whose connection drops has almost always
            // lost it to a daemon that went away, and the structure subscription notices
            // that and rebuilds every watcher through `follow` when it reconnects. Two
            // things retrying the same failure is how a dead daemon gets dialed sixteen
            // times a second.
            let Ok(mut stream) = connect(&socket_path, &subscription, &held) else {
                *poison::lock(&held, "watcher-stream") = None;
                return;
            };

            // Subscribed, and now ask what was missed on the way here. This thread is spawned
            // when the structure stream says the pane exists and dials afterwards, so a
            // transition landing in between reaches nobody and herdr has no replay for it -
            // the pane would keep its old state and look calm. One request per pane, once, at
            // creation (`Mirror::seed_agent_state`).
            for change in seed(&socket_path, &pane, &mirror, &names) {
                report(Notice::Changed(change));
            }

            let mut decoder = EventDecoder::new(names.clone());
            let mut buffer = [0u8; 1024];
            while let Ok(read) = stream.read(&mut buffer) {
                if read == 0 {
                    return;
                }
                let events = decoder.consume(&buffer[..read]);
                let changes: Vec<Change> = {
                    let mut mirror = poison::lock(&mirror, "mirror");
                    events.into_iter().flat_map(|event| mirror.apply(event)).collect()
                };
                for change in changes {
                    report(Notice::Changed(change));
                }
            }
        });

        Watcher { stream: slot }
    }
}

/// Asks the daemon what one pane's agent is doing, and takes the answer if nothing moved.
///
/// Its own connection rather than the subscription's, because that stream is a stream: the
/// reply to a request written onto it would arrive interleaved with events, and the decoder
/// reading it is not looking for one.
///
/// Every refusal is silence. A pane that closed while this was in flight, a daemon that went
/// away, an answer with no status in it - none of them are worth a log line per pane per
/// connection, and the state that results is the one the watcher would have had anyway.
fn seed(
    socket_path: &str,
    pane: &PaneId,
    mirror: &Arc<Mutex<Mirror>>,
    names: &Names,
) -> Vec<Change> {
    read_agent_state(socket_path, pane, mirror, names).unwrap_or_default()
}

fn read_agent_state(
    socket_path: &str,
    pane: &PaneId,
    mirror: &Arc<Mutex<Mirror>>,
    names: &Names,
) -> Option<Vec<Change>> {
    // Read before asking, so the answer can be refused if the subscription overtook it.
    let expected = poison::lock(mirror, "mirror").agent_state(pane)?;

    let answer = HerdrClient::new(socket_path)
        .request("pane.get", &json!({ "pane_id": names.backend_pane(pane).ok()?.as_str() }))
        .ok()?;
    // `{"type":"pane_info","pane":{..,"agent_status":".."}}`, with the outer `result`
    // already unwrapped by the client.
    let state = AgentState::from_backend(answer.get("pane")?.get("agent_status")?.as_str()?);

    let mut held = poison::lock(mirror, "mirror");
    Some(held.seed_agent_state(pane, state, Some(expected)))
}
