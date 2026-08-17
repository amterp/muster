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

use crate::client::HerdrClient;
use crate::events::EventDecoder;
use crate::snapshot::fetch_snapshot;

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
pub const STRUCTURE: [&str; 16] = [
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "workspace.focused",
    "tab.created",
    "tab.renamed",
    "tab.closed",
    "tab.focused",
    "pane.created",
    "pane.updated",
    "pane.closed",
    "pane.exited",
    "pane.focused",
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

        let handle = Subscription { running: running.clone(), stream: stream.clone() };
        std::thread::Builder::new()
            .name("muster-subscription".to_string())
            .spawn(move || run(&socket_path, &mirror, &report, &running, &stream, &names))
            .expect("could not start the subscription thread");
        handle
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

    let structure: Vec<Value> = STRUCTURE.iter().map(|kind| json!({ "type": kind })).collect();
    let mut agents = AgentWatchers::new(socket_path, mirror, report, names);

    while running.load(Ordering::Relaxed) {
        match connect(socket_path, &structure) {
            Ok(stream) => {
                *poison::lock(shared_stream, "subscription-stream") =
                    Some(stream.try_clone().expect("could not share the subscription"));
                if connected_before {
                    report(Notice::Reconnected);
                }
                connected_before = true;
                attempt = 0;

                // Snapshot after subscribing, never before. Between a snapshot and a
                // later subscribe there is a window in which an event fires and reaches
                // nobody, and it is invisible: the mirror would be wrong in a way no
                // counter reports. Subscribing first makes the overlap a duplicate
                // instead, which upsert already absorbs.
                bootstrap(socket_path, mirror, report, names);
                agents.follow();
                let detail = stream_events(stream, mirror, report, running, &mut agents, names);

                *poison::lock(shared_stream, "subscription-stream") = None;
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                poison::lock(mirror, "mirror").mark_stale();
                report(Notice::Stale { detail });
            }
            Err(detail) => {
                {
                    // Disconnected rather than stale on a failed dial: nothing has been
                    // reached, so there is no last good answer to label as aging.
                    let mut mirror = poison::lock(mirror, "mirror");
                    if connected_before { mirror.mark_stale() } else { mirror.mark_disconnected() }
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
fn connect(socket_path: &str, subscriptions: &[Value]) -> Result<UnixStream, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| error.to_string())?;
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

fn bootstrap(socket_path: &str, mirror: &Arc<Mutex<Mirror>>, report: &Report, names: &Names) {
    let Ok((snapshot, dropped)) = fetch_snapshot(socket_path, names) else { return };
    let changes = poison::lock(mirror, "mirror").bootstrap(snapshot);
    report(Notice::Bootstrapped { changes, dropped });
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
        let Ok(backend) = names.backend(pane) else { return Watcher { stream: slot } };
        let subscription =
            vec![json!({ "type": "pane.agent_status_changed", "pane_id": backend.as_str() })];
        let pane = pane.clone();

        let _ = std::thread::Builder::new().name(format!("muster-agent-{pane}")).spawn(move || {
            // No retry loop of its own. A watcher whose connection drops has almost always
            // lost it to a daemon that went away, and the structure subscription notices
            // that and rebuilds every watcher through `follow` when it reconnects. Two
            // things retrying the same failure is how a dead daemon gets dialed sixteen
            // times a second.
            let Ok(mut stream) = connect(&socket_path, &subscription) else { return };
            if let Ok(shared) = stream.try_clone() {
                *poison::lock(&held, "watcher-stream") = Some(shared);
            }

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
        .request("pane.get", &json!({ "pane_id": names.backend(pane).ok()?.as_str() }))
        .ok()?;
    // `{"type":"pane_info","pane":{..,"agent_status":".."}}`, with the outer `result`
    // already unwrapped by the client.
    let state = AgentState::from_backend(answer.get("pane")?.get("agent_status")?.as_str()?);

    let mut held = poison::lock(mirror, "mirror");
    Some(held.seed_agent_state(pane, state, Some(expected)))
}
