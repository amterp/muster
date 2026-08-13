//! What this process is holding open: daemons, their mirrors, and the attached panes.
//!
//! The runtime half of composition. Which daemons are attached and what each region shows
//! is a record in the core, judged by `composition.json` with no socket in sight; what is
//! here is the part that genuinely needs one - a held-open subscription per daemon, a bound
//! socket per pane, and the threads behind both.
//!
//! Keyed by daemon and by pane throughout, because both are plural. One window can show a
//! laptop and a devenv side by side, and two daemons hand out the same pane ids - `w1:p1`
//! means something on each - so a map keyed by pane alone would let one daemon's pane
//! answer for another's.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};

use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint};
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{Keymap, PaneInput, TerminalModeProfile};
use muster_core::mirror::backend::{PaneId, Snapshot};
use muster_core::mirror::{Change, Mirror};
use muster_herdr::subscription::{Notice, Subscription};
use muster_herdr::{HerdrPaneChannel, PaneControlChannel, discover_socket_path, fetch_snapshot};
use muster_vt::KeyEncoder;

use crate::ffi;
use crate::proto::{BackendHealth, Event, PaneStateChanged, PaneTypeable, event};

/// What Muster calls the daemon it found for itself.
///
/// A placeholder for configuration rather than a fact about the daemon: a config file will
/// name the ones it lists, and this is the name for the one nobody named. It stops being
/// the only entry the day an SSH endpoint is attached beside it.
const LOCAL: &str = "local";

/// Everything one attached pane needs to be typed into.
#[derive(Debug)]
pub(crate) struct AttachedPane {
    pub(crate) input: PaneInput,
    pub(crate) control_socket_path: String,
    pub(crate) server_encoded: bool,
    /// Held because dropping it unlinks the socket and stops the listener.
    _control: Arc<PaneControlChannel>,
}

/// One daemon this process is following.
#[derive(Debug)]
struct Backend {
    mirror: Arc<Mutex<Mirror>>,
    /// Held because dropping it ends the subscription and every thread under it - the
    /// structure stream and one agent watcher per pane.
    _subscription: Subscription,
}

/// Everything this process holds open, and the composition it holds it for.
#[derive(Debug, Default)]
pub(crate) struct Session {
    composition: Composition,
    backends: BTreeMap<DaemonId, Backend>,
    /// Nested rather than keyed by a pair, so that finding the pane a keystroke is for costs
    /// two lookups and no allocation - a pair key would have to be built, and building one
    /// means cloning both ids on a path that runs per keystroke.
    panes: BTreeMap<DaemonId, BTreeMap<PaneId, Arc<AttachedPane>>>,
    /// Names the next pane's socket. A counter rather than the pane's id: a Unix socket path
    /// has about a hundred bytes to spend and the temporary directory has already spent half
    /// of them, and a backend is free to spell an id with characters a path cannot hold.
    next_socket: u64,
}

pub(crate) static SESSION: LazyLock<Mutex<Session>> =
    LazyLock::new(|| Mutex::new(Session::default()));

impl Session {
    /// Starts following a daemon, or leaves the one already being followed alone.
    ///
    /// Seeded with a snapshot the caller already has, before the subscription starts. The
    /// subscription takes its own moments later and replaces this one; doing it in the other
    /// order would let the older answer land last and stick, since a mirror with nothing
    /// happening in it is never corrected.
    fn follow(&mut self, daemon: &DaemonId, socket_path: &str, seed: Snapshot) {
        self.composition.attach_daemon(Daemon {
            id: daemon.clone(),
            endpoint: Endpoint::Local { socket_path: socket_path.to_string() },
        });
        if self.backends.contains_key(daemon) {
            return;
        }

        let mut mirror = Mirror::new();
        mirror.bootstrap(seed);
        let mirror = Arc::new(Mutex::new(mirror));

        let reporting = daemon.clone();
        let subscription = Subscription::start(
            socket_path,
            Arc::clone(&mirror),
            Arc::new(move |notice| announce(&reporting, notice)),
        );
        self.backends.insert(daemon.clone(), Backend { mirror, _subscription: subscription });
    }

    /// Brings composition, and what this process holds open, in line with one daemon.
    ///
    /// The pane map is trimmed here as well as the regions, because every entry in it owns a
    /// bound socket and the thread waiting on it. A window whose panes come and go all day
    /// would otherwise collect both, and neither shows up as anything but a process that
    /// grows.
    fn reconcile(&mut self, daemon: &DaemonId) {
        let Some(backend) = self.backends.get(daemon) else { return };
        let Ok(mirror) = backend.mirror.lock() else { return };

        self.composition.reconcile(daemon, &mirror);
        if let Some(attached) = self.panes.get_mut(daemon) {
            attached.retain(|pane, _| mirror.pane(pane).is_some());
        }
    }

    /// The pane this window's keyboard feeds.
    ///
    /// Handed back behind an `Arc` so the caller can let go of this lock before it sends
    /// anything. A send can be a round trip to a daemon, and holding the session across one
    /// would stall every event arriving from every other daemon behind a wedged one.
    fn keyboard_pane(&self) -> Option<Arc<AttachedPane>> {
        let region = self.composition.focused_region()?;
        self.panes.get(&region.daemon)?.get(region.pane.as_ref()?).map(Arc::clone)
    }

    fn next_socket_path(&mut self) -> String {
        // A pid in the name because nothing else can legitimately own this path, which is
        // what makes unlinking a stale one safe.
        let name = format!("muster-{}-{}.sock", std::process::id(), self.next_socket);
        self.next_socket += 1;
        std::env::temp_dir().join(name).to_string_lossy().into_owned()
    }
}

/// The pane this window's keyboard feeds, if it has one.
pub(crate) fn keyboard_pane() -> Option<Arc<AttachedPane>> {
    SESSION.lock().expect("a panicking sender poisoned the session").keyboard_pane()
}

/// Why a pane could not be attached.
pub(crate) enum AttachError {
    NoDaemon,
    Unreachable(String),
    NoSuchPane { pane: String, held: usize, dropped: usize },
    NoSocket(String),
    NoEncoder(String),
}

/// Shows a daemon-owned pane in this window, and points the keyboard at it.
///
/// The daemon is asked where the pane lives before anything is built, because the answer
/// decides whether there is anything to build. A region shows a tab and only the daemon
/// knows which tab a pane is in - and a window that attaches to a pane no daemon holds is
/// the failure that has cost this project the most time, because it looks exactly like a
/// window that renders and ignores the keyboard.
///
/// The socket is bound before this returns, and so before the shell creates the surface
/// that spawns the bridge - which is what stops the bridge losing a race against its own
/// listener.
pub(crate) fn attach(pane_id: &str) -> Result<Arc<AttachedPane>, AttachError> {
    let pane = PaneId::new(pane_id);
    let socket_path =
        discover_socket_path(&std::env::vars().collect()).ok_or(AttachError::NoDaemon)?;

    let (snapshot, dropped) = fetch_snapshot(&socket_path)
        .map_err(|failure| AttachError::Unreachable(failure.to_string()))?;
    let Some(placed) = snapshot.panes.iter().find(|held| held.id == pane) else {
        return Err(AttachError::NoSuchPane {
            pane: pane_id.to_string(),
            held: snapshot.panes.len(),
            dropped,
        });
    };
    let (workspace, tab) = (placed.workspace.clone(), placed.tab.clone());

    let daemon = DaemonId::new(LOCAL);
    let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
    session.follow(&daemon, &socket_path, snapshot);

    let path = session.next_socket_path();
    let announced = pane.clone();
    let control = PaneControlChannel::bind(path.clone(), move || typeable(&announced))
        .map_err(|error| AttachError::NoSocket(error.to_string()))?;
    let control = Arc::new(control);

    // The second channel, for the keys and text whose correct encoding depends on modes the
    // control stream cannot show us.
    let server = HerdrPaneChannel::discover(pane_id);
    if server.is_none() {
        log::warn(
            "app.server_channel.unavailable",
            fields! {
                "impact" => "arrow keys and paste fall back to a guessed encoding, which \
                             pagers reject and multi-line pastes run as commands",
            },
        );
    }
    let server_encoded = server.is_some();

    // The pane's modes are not readable, so this is the documented guess. One day it is fed
    // from the daemon; nothing above here changes when it is.
    let encoder = KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE)
        .map_err(|error| AttachError::NoEncoder(error.to_string()))?;

    let attached = Arc::new(AttachedPane {
        input: PaneInput::new(
            Arc::clone(&control) as Arc<_>,
            server.map(|channel| Arc::new(channel) as Arc<_>),
            Arc::new(encoder),
            Keymap::default(),
        ),
        control_socket_path: path,
        server_encoded,
        _control: control,
    });
    session.panes.entry(daemon.clone()).or_default().insert(pane.clone(), Arc::clone(&attached));

    // One region per tab, not per pane. A tab's panes are the tab's own tree and they are
    // rendered inside one region; attaching a second pane from a tab already on screen is
    // asking for the keyboard, not for a second copy of the tab.
    let region = match session.composition.region_showing(&daemon, &tab) {
        Some(region) => region,
        None => session
            .composition
            .open_region(&daemon, workspace, tab)
            .expect("the daemon was attached one line above this"),
    };
    session.composition.focus_pane(region, pane);

    Ok(attached)
}

/// Applies what a daemon just said to what Muster is holding open.
///
/// Separate from reporting it, and finished before reporting starts: reporting reaches the
/// shell, the shell reacts by dispatching, and a dispatch that arrived while this held the
/// session would deadlock against it on the same thread.
fn reconcile(daemon: &DaemonId) {
    let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
    session.reconcile(daemon);
}

/// Turns what one daemon said into a log line and, where the window renders it, an event.
///
/// The whole of D's answer to "agent states are the point": every pane's transitions reach
/// the log, and the attached one's reaches the chrome. Which pane the window is showing is
/// the shell's business, so every pane is sent and the shell decides.
fn announce(daemon: &DaemonId, notice: Notice) {
    match notice {
        Notice::Bootstrapped { changes, dropped } => {
            log::info(
                "mirror.bootstrap",
                fields! {
                    "daemon" => daemon.to_string(),
                    "changes" => changes.len().to_string(),
                    "dropped" => dropped.to_string(),
                },
            );
            if dropped > 0 {
                log::warn(
                    "mirror.entries_dropped",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "count" => dropped.to_string(),
                        "impact" => "the session renders with fewer panes than the daemon \
                                     holds, which looks like panes the user closed",
                        "check" => "a herdr whose pane, tab or workspace payload has moved - \
                                    compare corpus/herdr-<version>/api-schema.json",
                    },
                );
            }
            // A bootstrap replaces the whole picture, so anything composition names may have
            // gone in the gap it was rebuilt across.
            reconcile(daemon);
            health("connected", "");
            for change in changes {
                report(&change);
            }
        }
        Notice::Changed(change) => {
            if moves_structure(&change) {
                reconcile(daemon);
            }
            report(&change);
        }
        Notice::Stale { detail } => {
            // Deliberately not a detach. A stale daemon is one Muster expects back, and its
            // regions are the last true thing anyone knows about it - closing them would
            // empty the window every time a laptop's lid shut (`architecture.md`,
            // degradation).
            log::warn(
                "backend.stale",
                fields! {
                    "daemon" => daemon.to_string(),
                    "detail" => detail.clone(),
                    "impact" => "panes keep rendering and the agent states shown are now a \
                                 guess about the present",
                    "check" => "whether the daemon is still running, and for a remote one \
                                whether the tunnel is up",
                },
            );
            health("stale", &detail);
        }
        Notice::Reconnected => {
            log::info("backend.reconnected", fields! { "daemon" => daemon.to_string() });
            health("connected", "");
        }
        Notice::UnknownEvent { kind } => log::warn(
            "backend.unknown_event",
            fields! {
                "daemon" => daemon.to_string(),
                "kind" => kind,
                "impact" => "whatever this event reports is not reaching the mirror, so the \
                             view is missing that kind of change entirely",
                "check" => "whether this herdr is newer than the pinned one - if the event \
                            matters, it needs reading in muster-herdr's decoder",
            },
        ),
    }
}

/// Whether this change can have moved something composition names.
///
/// Agent state and daemon focus cannot: one is a property of a pane that still exists, and
/// the other is a cursor Muster writes and never reads. Everything else moves a tab or a
/// pane, and both are things a region is holding on to.
fn moves_structure(change: &Change) -> bool {
    !matches!(
        change,
        Change::AgentStateChanged { .. }
            | Change::AgentTransitionsMissed { .. }
            | Change::FocusChanged
    )
}

fn report(change: &Change) {
    if let Change::AgentStateChanged { pane, from, to } = change {
        log::info(
            "agent.state",
            fields! {
                "pane" => pane.to_string(),
                "from" => from.as_str(),
                "to" => to.as_str(),
            },
        );
        ffi::emit(&Event {
            payload: Some(event::Payload::PaneStateChanged(PaneStateChanged {
                pane_id: pane.to_string(),
                state: to.as_str().to_string(),
            })),
        });
    }
}

fn health(state: &str, detail: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::BackendHealth(BackendHealth {
            state: state.to_string(),
            detail: detail.to_string(),
        })),
    });
}

/// The moment the pane becomes typeable, on the thread that accepted the connection.
fn typeable(pane: &PaneId) {
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneTypeable(PaneTypeable { pane_id: pane.to_string() })),
    });
}
