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
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use muster_core::composition::{Composition, Daemon, DaemonId, Endpoint, RegionId, Step, View};
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{Keymap, PaneInput, TerminalModeProfile};
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::mirror::backend::{PaneId, Snapshot};
use muster_core::mirror::{Change, Mirror};
use muster_herdr::subscription::{Notice, Subscription};
use muster_herdr::{
    HerdrClient, HerdrPaneChannel, PaneControlChannel, discover_socket_path, fetch_snapshot,
};
use muster_vt::KeyEncoder;

use crate::proto::{BackendHealth, Event, PaneStateChanged, PaneTypeable, event};
use crate::{convert, ffi};

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
    /// Held because dropping it unlinks the socket and stops the listener.
    _control: Arc<PaneControlChannel>,
}

/// One daemon this process is following.
#[derive(Debug)]
struct Backend {
    mirror: Arc<Mutex<Mirror>>,
    /// Where this daemon was actually found, as opposed to how it was asked for.
    ///
    /// The resolution rather than the wish, which is why it lives here and not in the
    /// composition record beside it: a path discovered from this run's environment, or
    /// forwarded from another machine, describes nothing a later run could use.
    socket_path: String,
    /// How this daemon is asked for changes. One per daemon rather than one per pane,
    /// because what these ask for is structure and structure belongs to the daemon.
    channel: Arc<dyn BackendChannel>,
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
    ///
    /// The endpoint and the socket path are both passed because they are different things.
    /// The endpoint is what someone asked for and is what composition writes down; the path
    /// is where this run found it, and is worth nothing to a later one.
    fn follow(&mut self, daemon: &DaemonId, endpoint: Endpoint, socket_path: &str, seed: Snapshot) {
        self.composition.attach_daemon(Daemon { id: daemon.clone(), endpoint });
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
        self.backends.insert(
            daemon.clone(),
            Backend {
                mirror,
                socket_path: socket_path.to_string(),
                channel: Arc::new(HerdrClient::new(socket_path)),
                _subscription: subscription,
            },
        );
    }

    /// Brings composition, and what this process holds open, in line with one daemon.
    ///
    /// A channel per pane in every region this daemon shows, not only the one the keyboard
    /// is on: the view names a socket for each leaf and a shell renders a surface per leaf,
    /// so a pane the daemon added is one Muster has to be ready to be typed into before
    /// anyone looks at it.
    ///
    /// The same pass drops channels for panes that are gone, because each one owns a bound
    /// socket and the thread waiting on it. A window whose panes come and go all day would
    /// otherwise collect both, and neither shows up as anything but a process that grows.
    fn reconcile(&mut self, daemon: &DaemonId) {
        // In two passes, because opening a channel needs the whole session and reading the
        // mirror borrows one daemon out of it. Nothing can change in between: the caller
        // holds the session across both.
        let wanted: Vec<PaneId> = {
            let Some(backend) = self.backends.get(daemon) else { return };
            let Ok(mirror) = backend.mirror.lock() else { return };

            self.composition.reconcile(daemon, &mirror);
            let attached = self.panes.entry(daemon.clone()).or_default();
            attached.retain(|pane, _| mirror.pane(pane).is_some());

            self.composition
                .regions()
                .filter(|region| &region.daemon == daemon)
                .filter_map(|region| mirror.layout(&region.tab))
                .flat_map(|layout| layout.root.panes())
                .filter(|pane| !attached.contains_key(pane) && mirror.pane(pane).is_some())
                .cloned()
                .collect()
        };

        for pane in wanted {
            if let Err(refusal) = self.open_channel(daemon, &pane) {
                // Logged rather than returned: nothing called this to attach that pane, and
                // the pane still renders. What it costs is that one pane's keyboard, so the
                // line has to name it.
                log::error(
                    "pane.channel.unavailable",
                    fields! {
                        "daemon" => daemon.to_string(),
                        "pane" => pane.to_string(),
                        "detail" => refusal,
                        "impact" => "this pane renders and ignores the keyboard; every other \
                                     pane in the window is unaffected",
                    },
                );
            }
        }
    }

    /// Binds a pane's socket and builds its input path.
    ///
    /// The socket is bound before this returns, and so before the shell is told about the
    /// pane it belongs to - which is what stops a bridge losing a race against its own
    /// listener.
    fn open_channel(&mut self, daemon: &DaemonId, pane: &PaneId) -> Result<(), String> {
        if self.panes.get(daemon).is_some_and(|held| held.contains_key(pane)) {
            return Ok(());
        }
        let socket_path =
            self.backends.get(daemon).map(|backend| backend.socket_path.clone()).ok_or_else(
                || {
                    format!(
                        "the daemon {daemon} is not being followed, so there is nowhere to send \
                     this pane's input. This is a bug in the core rather than a state to \
                     recover from: a channel is only ever opened for a daemon already \
                     attached."
                    )
                },
            )?;
        let path = self.next_socket_path();
        let (announced_daemon, announced_pane) = (daemon.clone(), pane.clone());
        let control = PaneControlChannel::bind(path.clone(), move || {
            typeable(&announced_daemon, &announced_pane);
        })
        .map_err(|error| {
            format!(
                "could not bind the socket this pane's bridge dials back on ({error}). \
                     Usual causes: a full or read-only temporary directory."
            )
        })?;
        let control = Arc::new(control);

        // The second channel, for the keys and text whose correct encoding depends on modes
        // the control stream cannot show us. Pointed at the daemon this pane belongs to
        // rather than at whatever the environment names: a remote pane asked of the local
        // daemon is a pane whose arrows quietly go to the wrong machine, and the failure
        // reads as a guessed encoding rather than as an error.
        let server = HerdrPaneChannel::new(HerdrClient::new(socket_path), pane.as_str());

        // The pane's modes are not readable, so this is the documented guess. One day it is
        // fed from the daemon; nothing above here changes when it is.
        let encoder = KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE).map_err(|error| {
            format!(
                "could not build a key encoder ({error}), so nothing typed into this pane \
                 would reach it. libghostty-vt is behind this; check that ./dev built it."
            )
        })?;

        self.panes.entry(daemon.clone()).or_default().insert(
            pane.clone(),
            Arc::new(AttachedPane {
                input: PaneInput::new(
                    Arc::clone(&control) as Arc<_>,
                    Some(Arc::new(server) as Arc<_>),
                    Arc::new(encoder),
                    Keymap::default(),
                ),
                control_socket_path: path,
                _control: control,
            }),
        );
        Ok(())
    }

    fn channel(&self, daemon: &DaemonId, pane: &PaneId) -> Option<&Arc<AttachedPane>> {
        self.panes.get(daemon)?.get(pane)
    }

    /// What this window is showing, right now.
    ///
    /// Every daemon's mirror is locked for the length of it, in the map's own order. That
    /// order is what makes it safe: the only other path taking two of these locks takes them
    /// one at a time and lets go of the mirror before it asks for the session.
    fn view(&self) -> View {
        let mirrors: BTreeMap<&DaemonId, MutexGuard<'_, Mirror>> = self
            .backends
            .iter()
            .filter_map(|(id, backend)| Some((id, backend.mirror.lock().ok()?)))
            .collect();
        View::of(
            &self.composition,
            |daemon| mirrors.get(daemon).map(|held| &**held),
            |daemon, pane| self.channel(daemon, pane).map(|held| held.control_socket_path.clone()),
        )
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

    /// Which of one daemon's regions shows this pane.
    ///
    /// Scoped to a daemon rather than searched across all of them, because two daemons hand
    /// out the same pane ids - `w1:p1` means something on each - and a search would let
    /// whichever happened to be first answer for the other's pane. A pane in none of that
    /// daemon's regions is one this window is not showing, and nothing here will act on it.
    fn region_holding(&self, daemon: &DaemonId, pane: &PaneId) -> Option<RegionId> {
        let backend = self.backends.get(daemon)?;
        let held = backend.mirror.lock().ok()?;
        self.composition
            .regions()
            .find(|region| {
                &region.daemon == daemon
                    && held.pane(pane).is_some_and(|held| held.tab == region.tab)
            })
            .map(|region| region.id)
    }

    fn channel_of(&self, daemon: &DaemonId) -> Option<Arc<dyn BackendChannel>> {
        self.backends.get(daemon).map(|backend| Arc::clone(&backend.channel))
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

/// Asks the daemon showing this window for a change.
///
/// Nothing is applied here, and nothing is answered with new state: what happened arrives
/// afterwards on the daemon's own events, which is what keeps one description of the session
/// rather than two that can disagree (`architecture.md`, view = f(daemon state)).
///
/// The one exception is where the keyboard ends up, which is Muster's own state and not the
/// daemon's. A split you asked for takes it, because that is what pressing the key meant.
///
/// The channel is taken out from under the lock before the request goes, because a request
/// is a round trip and holding the session across one would stall every event arriving from
/// every other daemon behind a wedged one.
pub(crate) fn submit(daemon: &DaemonId, intent: &BackendIntent) -> Result<(), String> {
    let (region, channel) = {
        let session = SESSION.lock().expect("a panicking sender poisoned the session");
        let found = match intent {
            BackendIntent::SplitPane { pane, .. }
            | BackendIntent::ClosePane { pane }
            | BackendIntent::FocusPane { pane } => session.region_holding(daemon, pane),
            BackendIntent::SetSplitRatio { tab, .. } => {
                session.composition.region_showing(daemon, tab)
            }
        };
        let region = found.ok_or_else(|| {
            format!(
                "the daemon {daemon} is not showing that pane or tab in this window, so \
                 nothing was asked of anything. Either it closed while this was in flight, \
                 or the request names something in a session this window is not attached to."
            )
        })?;
        let channel = session.channel_of(daemon).ok_or_else(|| {
            format!(
                "the daemon {daemon} is in this window's composition and is not being \
                     followed, which is a bug in the core rather than a state to recover from"
            )
        })?;
        (region, channel)
    };

    let outcome = channel.submit(intent);
    log::info(
        "intent.submitted",
        fields! {
            "intent" => format!("{intent:?}"),
            "backend" => channel.description(),
            "created" => outcome.as_ref().ok().and_then(|outcome| outcome.created.clone())
                .map(|pane| pane.to_string()).unwrap_or_default(),
            "refused" => outcome.as_ref().err().cloned().unwrap_or_default(),
        },
    );

    // The new pane is not in the mirror yet - its event is still in flight - so the region is
    // the one that was split rather than one looked up. Taking the pane on trust is what
    // `Composition::focus_pane` is for, and the reconcile behind the event that follows is
    // where daemon truth gets applied to it.
    if let Some(created) = outcome.as_ref().ok().and_then(|outcome| outcome.created.clone()) {
        SESSION
            .lock()
            .expect("a panicking sender poisoned the session")
            .composition
            .focus_pane(region, created);
        publish();
    }
    outcome.map(|_| ())
}

/// Points this window's keyboard at a pane, and tells the daemon somebody looked.
///
/// The two halves are one action to whoever clicked, and they are kept separate underneath:
/// the keyboard moves whatever the daemon says, because it is Muster's own cursor, and the
/// daemon is told as a courtesy it may refuse. A refused write is worth a log line and not
/// worth undoing a focus move the user can see happened.
pub(crate) fn focus(daemon: &DaemonId, pane: &PaneId) -> Result<(), String> {
    {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        let region = session.region_holding(daemon, pane).ok_or_else(|| {
            format!(
                "no region in this window is showing a pane called {pane} on {daemon}, so \
                 the keyboard stayed where it was. Either the pane closed while this was in \
                 flight, or it belongs to a tab this window is not showing."
            )
        })?;
        session.composition.focus_pane(region, pane.clone());
    }
    publish();
    submit(daemon, &BackendIntent::FocusPane { pane: pane.clone() })
}

/// Moves the keyboard one pane along, in the window's own reading order.
///
/// The order crosses regions, so a step can land on another daemon - which is the point of
/// showing two of them - and the daemon comes back with the region rather than being assumed
/// to be the one the keyboard just left.
pub(crate) fn step(direction: Step) -> Result<(), String> {
    let stepped = {
        let session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.view().step(direction).and_then(|(region, pane)| {
            Some((session.composition.region(region)?.daemon.clone(), pane))
        })
    };
    let (daemon, pane) = stepped.ok_or_else(|| {
        "this window is showing no panes to step through, so the keyboard stayed where it \
         was. A window with no daemon behind it looks like this, and so does one whose tabs \
         all closed."
            .to_string()
    })?;
    focus(&daemon, &pane)
}

/// The pane this window's keyboard feeds, named.
pub(crate) fn focused_pane() -> Option<PaneId> {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    session.composition.focused_region()?.pane.clone()
}

/// The daemon this window's keyboard is on.
///
/// What a request naming no daemon means, for the same reason an empty pane id means the
/// focused pane: a menu item is about what is in front of the user and has nothing else to
/// say.
pub(crate) fn focused_daemon() -> Option<DaemonId> {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    session.composition.focused_region().map(|region| region.daemon.clone())
}

/// Why a pane could not be attached.
pub(crate) enum AttachError {
    NoDaemon,
    Unreachable(String),
    NoSuchPane { pane: String, held: usize, dropped: usize },
    NoChannel(String),
}

/// Shows a daemon-owned pane in this window, and points the keyboard at it.
///
/// The daemon is asked where the pane lives before anything is built, because the answer
/// decides whether there is anything to build. A region shows a tab and only the daemon
/// knows which tab a pane is in - and a window that attaches to a pane no daemon holds is
/// the failure that has cost this project the most time, because it looks exactly like a
/// window that renders and ignores the keyboard.
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
    let attached = {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        // Nothing named this daemon, so nothing to record but the wish that produced it:
        // find whatever is on this machine. A config file naming a socket says so instead.
        session.follow(&daemon, Endpoint::Local { socket_path: None }, &socket_path, snapshot);

        // This pane's channel by hand, before the rest. The reconcile below opens one for
        // every other pane in the tab and logs whatever refuses; this one has a caller
        // waiting on an answer, so its refusal is returned rather than written down.
        session.open_channel(&daemon, &pane).map_err(AttachError::NoChannel)?;

        // One region per tab, not per pane. A tab's panes are the tab's own tree and they
        // are rendered inside one region; attaching a second pane from a tab already on
        // screen is asking for the keyboard, not for a second copy of the tab.
        let region = match session.composition.region_showing(&daemon, &tab) {
            Some(region) => region,
            None => session
                .composition
                .open_region(&daemon, workspace, tab)
                .expect("the daemon was attached a few lines above this"),
        };
        session.composition.focus_pane(region, pane.clone());
        session.reconcile(&daemon);

        session
            .channel(&daemon, &pane)
            .map(Arc::clone)
            .ok_or_else(|| AttachError::NoChannel("the channel opened and then went".to_string()))?
    };

    // Outside the lock, because emitting reaches the shell and a shell reacting to an event
    // by dispatching a request is ordinary.
    publish();
    Ok(attached)
}

/// Tells the shell what this window is showing.
///
/// The whole view rather than what moved. A shell handed the whole answer holds no picture
/// of its own to patch, and the message is a few hundred bytes for a window nobody can fill
/// past about fifteen panes.
fn publish() {
    let view = {
        let session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.view()
    };

    // The shape, not the fact. "the view changed" is useless in a bug report and what it
    // changed to is the whole answer - a window rendering the wrong thing and a window
    // rendering nothing are one line apart here (`architecture.md`, the diagnostic log).
    for region in &view.regions {
        log::info(
            "view.region",
            fields! {
                "region" => region.id.to_string(),
                "daemon" => region.daemon.to_string(),
                "tab" => region.tab.to_string(),
                "keyboard" => region.pane.as_ref().map(ToString::to_string).unwrap_or_default(),
                "tree" => match &region.root {
                    Some(root) => root.to_string(),
                    None => "(not yet published)".to_string(),
                },
                "focused" => view.focused == Some(region.id),
            },
        );
    }
    ffi::emit(&Event { payload: Some(event::Payload::ViewChanged(convert::view(&view))) });
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
            publish();
            health(daemon, "connected", "");
            for change in changes {
                report(daemon, &change);
            }
        }
        Notice::Changed(change) => {
            if moves_structure(&change) {
                reconcile(daemon);
                publish();
            }
            report(daemon, &change);
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
            health(daemon, "stale", &detail);
        }
        Notice::Reconnected => {
            log::info("backend.reconnected", fields! { "daemon" => daemon.to_string() });
            health(daemon, "connected", "");
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

fn report(daemon: &DaemonId, change: &Change) {
    if let Change::AgentStateChanged { pane, from, to } = change {
        log::info(
            "agent.state",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "from" => from.as_str(),
                "to" => to.as_str(),
            },
        );
        ffi::emit(&Event {
            payload: Some(event::Payload::PaneStateChanged(PaneStateChanged {
                daemon_id: daemon.to_string(),
                pane_id: pane.to_string(),
                state: to.as_str().to_string(),
            })),
        });
    }
}

/// How much of one daemon's truth the core currently has.
///
/// Per daemon, because health is per connection. A window showing a laptop and a devenv has
/// two answers, and one of them going stale says nothing about the other - so a single
/// window-wide state would let a dropped VPN read as though every session had gone.
fn health(daemon: &DaemonId, state: &str, detail: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::BackendHealth(BackendHealth {
            daemon_id: daemon.to_string(),
            state: state.to_string(),
            detail: detail.to_string(),
        })),
    });
}

/// The moment the pane becomes typeable, on the thread that accepted the connection.
fn typeable(daemon: &DaemonId, pane: &PaneId) {
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneTypeable(PaneTypeable {
            daemon_id: daemon.to_string(),
            pane_id: pane.to_string(),
        })),
    });
}
