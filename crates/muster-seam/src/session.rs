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

use muster_core::AgentState;
use muster_core::attention::Attention;
use muster_core::composition::{
    Composition, Daemon, DaemonId, Endpoint, PaneKey, RegionId, Step, Transport, View,
};
use muster_core::config::Config;
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{Keymap, PaneInput, TerminalModeProfile};
use muster_core::intent::{BackendChannel, BackendIntent};
use muster_core::mirror::backend::{PaneId, Snapshot, TabId, WorkspaceId};
use muster_core::mirror::{Change, Mirror};
use muster_core::roster::Roster;
use muster_herdr::subscription::{Notice, Subscription};
use muster_herdr::{
    HerdrClient, HerdrPaneChannel, PaneControlChannel, daemon, discover_socket_path,
    fetch_snapshot, own_socket_path,
};
use muster_ssh::{Forward, Tunnel, remote_environment};
use muster_vt::KeyEncoder;

use crate::proto::{BackendHealth, Event, PaneStateChanged, PaneTypeable, event};
use crate::{convert, ffi};

/// What Muster calls the daemon it found for itself.
///
/// The name for the one nobody named. A config file that lists daemons names its own, and
/// this is what a config-less Muster calls the herdr on this machine.
const LOCAL: &str = "local";

/// The daemon binary this Muster ships, as the shell resolved it.
///
/// Held here rather than looked up, because where it sits is an OS and packaging question -
/// inside a bundle for a shipped app, beside the binary for a build - and the core answers
/// none of those. The shell hands it over at startup, the way it already does the log file
/// and the config file.
///
/// None means the shell found none, which is a real state and not a default to paper over: a
/// window with no daemon to start says so rather than rendering nothing in silence.
static DAEMON_BINARY: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_daemon_binary(path: &str) {
    let mut held = DAEMON_BINARY.lock().expect("a panicking sender poisoned the daemon binary");
    *held = if path.is_empty() { None } else { Some(path.to_string()) };
}

fn daemon_binary() -> Option<String> {
    DAEMON_BINARY.lock().expect("a panicking sender poisoned the daemon binary").clone()
}

/// A daemon's endpoint, turned into something that can be connected to.
///
/// The one place local and remote differ. Everything past this point holds a socket path and
/// never asks where it goes, which is the property that lets one adapter serve both.
#[derive(Debug)]
struct Reached {
    socket_path: String,
    tunnel: Option<Tunnel>,
}

/// Opens whatever a daemon's endpoint describes.
///
/// For a daemon on this machine that is a path, found the way herdr's own client finds it
/// when the config did not say. For a remote one it is an ssh master forwarding that
/// daemon's socket onto a path here - so the answer has the same shape either way, and the
/// mirror, the subscription and the encoder below never learn which they got.
fn reach(daemon: &DaemonId, endpoint: &Endpoint) -> Result<Reached, String> {
    match endpoint {
        // A socket somebody named is a daemon somebody chose, of a version nobody promised.
        // Taken as asked for, and left alone: this is the deliberate way out of the
        // arrangement below, and second-guessing it would leave no way out at all.
        Endpoint::Local { socket_path: Some(path) } => {
            Ok(Reached { socket_path: path.clone(), tunnel: None })
        }
        Endpoint::Local { socket_path: None } => {
            let path = own_socket_path(&daemon::environment()).ok_or_else(|| {
                "Muster cannot work out where its own daemon's socket would go, because \
                 nothing in the environment says where home is - neither HOME nor \
                 XDG_CONFIG_HOME. This window will render nothing. Give the daemon a `socket` \
                 in the config file to say outright."
                    .to_string()
            })?;
            daemon::ensure_running(&path, daemon_binary().as_deref())?;
            Ok(Reached { socket_path: path, tunnel: None })
        }
        // A remote is the one place Muster does not yet own its daemon, and the reason is
        // packaging rather than principle: the bundle carries a binary for this machine's
        // platform and a devenv is usually another. So an ssh endpoint attaches to whatever
        // herdr is installed over there, at whatever version. Closing that is a_28QlRpvKj.
        Endpoint::Ssh { host, options, socket_path } => {
            // Asked for rather than assumed, and asked for using the rules Muster already
            // has: a shell one-liner spelling out where herdr keeps its socket would be a
            // second copy of the thing most likely to drift.
            let remote = if let Some(path) = socket_path {
                path.clone()
            } else {
                let environment = remote_environment(host, options)?;
                discover_socket_path(&environment).ok_or_else(|| {
                    format!(
                        "{host} answered, and nothing in its environment says where its herdr \
                         socket would be - it has no HOME. Name the daemon's socket in the \
                         config file's `socket` key."
                    )
                })?
            };
            let tunnel = Tunnel::open(Forward {
                host: host.clone(),
                options: options.clone(),
                control_path: tunnel_path(daemon, "ctl"),
                local_socket: tunnel_path(daemon, "sock"),
                remote_socket: remote,
            })?;
            Ok(Reached {
                socket_path: tunnel.local_socket_path().to_string(),
                tunnel: Some(tunnel),
            })
        }
    }
}

/// Where a daemon's tunnel puts its ends.
///
/// Named for the daemon rather than numbered, unlike a pane's socket, because there are a
/// handful of these and the name is what makes one recognisable in `lsof` at the moment
/// somebody is wondering which connection is wedged. The pid keeps two Musters apart.
fn tunnel_path(daemon: &DaemonId, extension: &str) -> String {
    let name = format!("muster-{}-{daemon}.{extension}", std::process::id());
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

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
    /// The ssh master this daemon is reached through, for a remote one.
    ///
    /// Held because dropping it takes the connection down, and named because a pane's bridge
    /// needs the same master to run its frame stream through. Absent for a daemon on this
    /// machine, which is the difference the rest of this file never has to notice.
    tunnel: Option<Tunnel>,
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
    /// Which agents have been seen, and so which are `done`.
    ///
    /// Beside the mirrors rather than inside one, because it spans them: a window is focused
    /// or it is not, and that answers for a laptop's panes and a devenv's at once.
    attention: Attention,
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
    ///
    /// Returns what the seed established, for the caller to announce once it has let go of
    /// the session. Nothing else will: the subscription's own bootstrap diffs against this
    /// mirror rather than an empty one, so every pane the seed already placed is a pane it
    /// sees no change in. Dropping these on the floor is how a window opened onto running
    /// agents came up believing every one of them was unknown.
    fn follow(&mut self, daemon: &Daemon, reached: Reached, seed: Snapshot) -> Vec<Change> {
        let id = daemon.id.clone();
        self.composition.attach_daemon(daemon.clone());
        if self.backends.contains_key(&id) {
            return Vec::new();
        }

        let mut mirror = Mirror::new();
        let seeded = mirror.bootstrap(seed);
        let mirror = Arc::new(Mutex::new(mirror));

        let reporting = id.clone();
        let subscription = Subscription::start(
            &reached.socket_path,
            Arc::clone(&mirror),
            Arc::new(move |notice| announce(&reporting, notice)),
        );
        self.backends.insert(
            id,
            Backend {
                mirror,
                tunnel: reached.tunnel,
                channel: Arc::new(HerdrClient::new(reached.socket_path.clone())),
                socket_path: reached.socket_path,
                _subscription: subscription,
            },
        );
        seeded
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

    /// What one daemon's mirror says a pane's agent is doing.
    ///
    /// Scoped to the daemon rather than searched, for the reason every other lookup here is:
    /// two daemons hand out `w1:p1`, and a search would let one answer for the other's pane.
    fn agent_state(&self, pane: &PaneKey) -> Option<AgentState> {
        let mirror = self.backends.get(&pane.daemon)?.mirror.lock().ok()?;
        mirror.agent_state(&pane.pane)
    }

    /// Puts a region onto the tab holding this pane, so that something can show it.
    ///
    /// The mirror is what knows which tab a pane is in, so the lookup is here and the policy
    /// - which region, or a new one - is in the composition record where the rest of it is.
    ///
    /// Refuses by name rather than silently doing nothing. A pane the daemon has never heard
    /// of and a pane that closed while a click was in flight look identical from a sidebar
    /// row, and both leave the keyboard where it was.
    fn surface(&mut self, daemon: &DaemonId, pane: &PaneId) -> Result<RegionId, String> {
        let (workspace, tab) = {
            let mirror = self
                .backends
                .get(daemon)
                .ok_or_else(|| {
                    format!(
                        "this window is not following a daemon called {daemon}, so there is \
                         nothing to show {pane} in and the keyboard stayed where it was."
                    )
                })?
                .mirror
                .lock()
                .map_err(|_| {
                    format!("the mirror for {daemon} was poisoned by a panicking sender")
                })?;
            let held = mirror.pane(pane).ok_or_else(|| {
                format!(
                    "{daemon} holds no pane called {pane}, so the keyboard stayed where it \
                     was. Most likely it closed while this was in flight, which an entry in a \
                     list outlives by a moment."
                )
            })?;
            (held.workspace.clone(), held.tab.clone())
        };
        self.composition.surface(daemon, workspace, tab).ok_or_else(|| {
            format!(
                "{daemon} is followed but not attached to this window's composition, so no \
                 region could be opened onto {pane}."
            )
        })
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
            |daemon| {
                let tunnel = self.backends.get(daemon)?.tunnel.as_ref()?;
                Some(Transport {
                    host: tunnel.host().to_string(),
                    control_path: tunnel.control_path().to_string(),
                })
            },
            |daemon| {
                let backend = self.backends.get(daemon)?;
                // Only for a daemon on this machine. A remote one's socket path here is the
                // near end of a tunnel, and the bridge that would use it runs its CLI on the
                // far end, where that path names nothing at all.
                backend.tunnel.is_none().then(|| backend.socket_path.clone())
            },
        )
    }

    /// Everything the attached daemons hold, and which of it is on screen.
    ///
    /// Takes the view rather than recomputing what is visible, so the two answers cannot
    /// disagree - a row marked hidden while its surface is on screen is the sidebar being
    /// wrong about the window beside it.
    ///
    /// Locks every mirror for the length of it, in the map's own order, on the same terms as
    /// [`Session::view`].
    fn roster(&self, view: &View) -> Roster {
        let mirrors: BTreeMap<&DaemonId, MutexGuard<'_, Mirror>> = self
            .backends
            .iter()
            .filter_map(|(id, backend)| Some((id, backend.mirror.lock().ok()?)))
            .collect();
        Roster::of(
            &self.composition,
            |daemon| mirrors.get(daemon).map(|held| &**held),
            &view.showing(),
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
        // Which region this is about, for the keyboard afterwards. None is an answer rather
        // than a failure for an intent that names nothing existing - there is no region to
        // find for a workspace that does not exist yet, and the one it produces is opened by
        // the reconcile behind the daemon's own event.
        let region = match intent {
            BackendIntent::CreateWorkspace { .. } => None,
            BackendIntent::SplitPane { pane, .. }
            | BackendIntent::ClosePane { pane }
            | BackendIntent::FocusPane { pane } => {
                Some(session.region_holding(daemon, pane).ok_or_else(|| not_showing(daemon))?)
            }
            BackendIntent::SetSplitRatio { tab, .. } => Some(
                session
                    .composition
                    .region_showing(daemon, tab)
                    .ok_or_else(|| not_showing(daemon))?,
            ),
        };
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
    if let (Some(region), Some(created)) =
        (region, outcome.as_ref().ok().and_then(|outcome| outcome.created.clone()))
    {
        SESSION
            .lock()
            .expect("a panicking sender poisoned the session")
            .composition
            .focus_pane(region, created);
        publish();
    }
    outcome.map(|_| ())
}

/// Why an intent about something on screen could not be sent.
fn not_showing(daemon: &DaemonId) -> String {
    format!(
        "the daemon {daemon} is not showing that pane or tab in this window, so nothing was \
         asked of anything. Either it closed while this was in flight, or the request names \
         something in a session this window is not attached to."
    )
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
        let region = match session.region_holding(daemon, pane) {
            Some(region) => region,
            // Not on screen, which is the interesting half. An agent that finished or is
            // waiting for somebody is most often in a tab no region is showing, so a focus
            // request that refused there would leave the sidebar listing panes nobody can
            // reach - a display, not attention routing.
            None => session.surface(daemon, pane)?,
        };
        session.composition.focus_pane(region, pane.clone());
    }
    publish();
    submit(daemon, &BackendIntent::FocusPane { pane: pane.clone() })
}

/// Moves the line between two regions, and republishes what that made.
///
/// No daemon is told, and there is nothing to tell one: how a window divides itself between
/// a laptop and a devenv is Muster's own arrangement, and neither daemon knows the other
/// exists. So unlike every other drag in this app, this one settles here.
pub(crate) fn set_region_boundary(left: RegionId, ratio: f32) {
    {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.composition.set_boundary(left, ratio);
    }
    publish();
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

/// Starts following every daemon a config file named.
///
/// No regions yet. Which tab a region shows depends on where the pane in `argv` turned out to
/// live, and that is not known until [`attach`] has asked - so opening one here would mean
/// opening a second one a moment later and closing the first.
///
/// A daemon that will not attach is logged and skipped rather than fatal. One unreachable
/// devenv should cost its own panes and nothing else, and a window that refused to open
/// because a container was down would be worse than the herdr TUI it replaces.
pub(crate) fn follow_configured(config: &Config) {
    for daemon in &config.daemons {
        if let Err(refusal) = attach_daemon(daemon) {
            log::error(
                "daemon.unavailable",
                fields! {
                    "daemon" => daemon.id.to_string(),
                    "detail" => refusal,
                    "impact" => "this daemon's panes are absent from the window; every other \
                                 daemon in the config is unaffected",
                },
            );
        }
    }
}

/// Reaches one daemon, takes its first snapshot, and starts following it.
fn attach_daemon(daemon: &Daemon) -> Result<(), String> {
    let reached = reach(&daemon.id, &daemon.endpoint)?;
    let (snapshot, dropped) = fetch_snapshot(&reached.socket_path).map_err(|failure| {
        format!("the daemon {} did not answer at {} ({failure}).", daemon.id, reached.socket_path)
    })?;
    log::info(
        "daemon.attached",
        fields! {
            "daemon" => daemon.id.to_string(),
            "socket" => reached.socket_path.clone(),
            "remote" => reached.tunnel.as_ref().map_or("", Tunnel::host).to_string(),
            "panes" => snapshot.panes.len().to_string(),
            "dropped" => dropped.to_string(),
        },
    );
    let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
    let seeded = session.follow(daemon, reached, snapshot);
    // Before reporting, and explicitly: reporting reads the mirror back through the session
    // and reaches the shell, which may answer by dispatching. Both want this lock.
    drop(session);

    for change in seeded {
        report(&daemon.id, &change);
    }
    Ok(())
}

/// Why a pane could not be attached.
pub(crate) enum AttachError {
    Unreachable(String),
    NoSuchPane { pane: String, held: usize, dropped: usize },
    NoChannel(String),
}

/// Opens this window onto whatever the daemons hold.
///
/// What a bare `muster` means. No pane is named, so nothing decides which daemon or which tab
/// beyond what each daemon is already focused on - which is what its user was last looking at
/// and the best answer Muster has to invent.
///
/// Three steps, and each is the reason the next can be simple: be following something, give
/// every daemon a region, and make a workspace if all of that still leaves nothing to show.
/// The last is the one a fresh machine needs, where Muster has just started a daemon that
/// holds no panes at all.
pub(crate) fn open() -> Result<(), String> {
    follow_implicitly_if_nothing_else()?;
    open_remaining_regions();
    open_a_workspace_if_the_window_is_empty();
    publish();
    Ok(())
}

/// Attaches the daemon on this machine when no config file named any.
///
/// Recorded as the wish that produced it - Muster's own daemon, wherever that turns out to be
/// - rather than as the path that answered today.
fn follow_implicitly_if_nothing_else() -> Result<(), String> {
    if following_anything() {
        return Ok(());
    }
    let implicit =
        Daemon { id: DaemonId::new(LOCAL), endpoint: Endpoint::Local { socket_path: None } };
    attach_daemon(&implicit)
}

/// Asks for one workspace when nothing else has produced anything to show.
///
/// A daemon Muster started a moment ago holds nothing, so every rule above it produces an
/// empty window - which is the state this whole path exists to avoid. One workspace, on the
/// first daemon that is local, because a remote one is somebody else's machine and making
/// things on it uninvited is a bigger claim than filling a window.
///
/// Nothing is opened here. The daemon answers by publishing a workspace, a tab and a pane,
/// and the region appears the way every other region does - through the reconcile that
/// follows. A window that built one itself would be a second place layout is decided.
fn open_a_workspace_if_the_window_is_empty() {
    let empty = {
        let session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.composition.regions().next().is_none()
    };
    if !empty {
        return;
    }

    let Some(daemon) = first_local_daemon() else {
        log::warn(
            "window.empty",
            fields! {
                "impact" => "this window shows nothing, because no attached daemon holds a \
                             tab and none of them is on this machine",
                "check" => "whether the remote daemons in the config file have any sessions \
                            open - Muster will not make one on somebody else's machine",
            },
        );
        return;
    };

    log::info("workspace.creating", fields! { "daemon" => daemon.to_string() });
    if let Err(refusal) = submit(&daemon, &BackendIntent::CreateWorkspace { cwd: None }) {
        log::error(
            "workspace.refused",
            fields! {
                "daemon" => daemon.to_string(),
                "detail" => refusal,
                "impact" => "this window opens empty, and stays that way until something \
                             makes a pane on that daemon",
                "check" => "the daemon's own log - it answered its socket, so this is a \
                            refusal rather than an absence",
            },
        );
    }
}

/// The first attached daemon on this machine, in the order the config named them.
fn first_local_daemon() -> Option<DaemonId> {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    session
        .composition
        .daemons()
        .find(|daemon| matches!(daemon.endpoint, Endpoint::Local { .. }))
        .map(|daemon| daemon.id.clone())
}

/// Shows a daemon-owned pane in this window, and points the keyboard at it.
///
/// The daemon is asked where the pane lives before anything is built, because the answer
/// decides whether there is anything to build. A region shows a tab and only the daemon
/// knows which tab a pane is in - and a window that attaches to a pane no daemon holds is
/// the failure that has cost this project the most time, because it looks exactly like a
/// window that renders and ignores the keyboard.
///
/// Which daemon holds the pane is searched for rather than said, because at this moment
/// nobody knows: `argv` carries a pane id and a config file carries daemons, and the two are
/// joined here. Every daemon already being followed is asked; a Muster with no config has one
/// to ask, which it finds the way herdr's own client would.
pub(crate) fn attach(pane_id: &str) -> Result<Arc<AttachedPane>, AttachError> {
    let pane = PaneId::new(pane_id);
    follow_implicitly_if_nothing_else().map_err(AttachError::Unreachable)?;

    let (daemon, workspace, tab) = locate(&pane).ok_or_else(|| AttachError::NoSuchPane {
        pane: pane_id.to_string(),
        held: panes_followed(),
        dropped: 0,
    })?;

    let attached = {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");

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
                .expect("the daemon holding this pane is one being followed"),
        };
        session.composition.focus_pane(region, pane.clone());
        session.reconcile(&daemon);

        session
            .channel(&daemon, &pane)
            .map(Arc::clone)
            .ok_or_else(|| AttachError::NoChannel("the channel opened and then went".to_string()))?
    };

    // Every other daemon gets a region of its own, on whatever tab it is focused on. This is
    // what puts a laptop and a devenv side by side: `argv` decides where the keyboard starts
    // and the config decides what else is on screen.
    open_remaining_regions();

    // Outside the lock, because emitting reaches the shell and a shell reacting to an event
    // by dispatching a request is ordinary.
    publish();
    Ok(attached)
}

fn following_anything() -> bool {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    !session.backends.is_empty()
}

fn panes_followed() -> usize {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    session
        .backends
        .values()
        .filter_map(|backend| backend.mirror.lock().ok())
        .map(|mirror| mirror.panes().count())
        .sum()
}

/// Which followed daemon holds this pane, and where in it.
///
/// The first that has it, and an ambiguity nobody can resolve when two do: `w1:p1` on a
/// laptop and `w1:p1` on a devenv are different panes with one name, and a command line
/// carrying only the name has not said which. Named as a hazard here rather than silently
/// resolved, because the day it bites, the window will have attached the wrong machine.
fn locate(pane: &PaneId) -> Option<(DaemonId, WorkspaceId, TabId)> {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    let mut found: Option<(DaemonId, WorkspaceId, TabId)> = None;
    for (id, backend) in &session.backends {
        let Ok(mirror) = backend.mirror.lock() else { continue };
        let Some(held) = mirror.pane(pane) else { continue };
        if let Some((first, ..)) = &found {
            log::warn(
                "pane.ambiguous",
                fields! {
                    "pane" => pane.to_string(),
                    "daemons" => format!("{first}, {id}"),
                    "impact" => "the keyboard started on the first of them, which may be the \
                                 wrong machine",
                    "check" => "name the pane on the daemon you meant once the CLI can say \
                                which - a pane id alone does not",
                },
            );
            break;
        }
        found = Some((id.clone(), held.workspace.clone(), held.tab.clone()));
    }
    found
}

/// Gives every daemon with nothing on screen a region of its own.
///
/// On the daemon's own focused tab, because that is the one its user was last looking at and
/// Muster has no better answer to invent. A daemon that has published no tabs yet gets
/// nothing and is picked up by the next reconcile.
fn open_remaining_regions() {
    let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
    let wanted: Vec<(DaemonId, WorkspaceId, TabId)> = session
        .backends
        .iter()
        .filter(|(id, _)| !session.composition.regions().any(|region| &&region.daemon == id))
        .filter_map(|(id, backend)| {
            let mirror = backend.mirror.lock().ok()?;
            let tab = mirror.focus().tab.clone()?;
            let held = mirror.tab(&tab)?;
            Some((id.clone(), held.workspace.clone(), tab))
        })
        .collect();
    for (daemon, workspace, tab) in wanted {
        session.composition.open_region(&daemon, workspace, tab);
        session.reconcile(&daemon);
    }
}

/// Tells the shell what this window is showing.
///
/// The whole view rather than what moved. A shell handed the whole answer holds no picture
/// of its own to patch, and the message is a few hundred bytes for a window nobody can fill
/// past about fifteen panes.
fn publish() {
    // What the window is showing is also the answer to which agents have been seen, so the
    // two are settled together rather than left to drift. `settled` is the panes that were
    // waiting to be noticed and have now been - re-announced below, after the shell has been
    // handed the arrangement they appear in.
    let (view, roster, settled) = {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        let view = session.view();
        let roster = session.roster(&view);
        let settled = session.attention.showing(view.showing());
        (view, roster, settled)
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
    log::info(
        "roster.published",
        fields! {
            "panes" => roster.panes.len().to_string(),
            "on_screen" => roster.panes.iter().filter(|pane| pane.on_screen).count().to_string(),
        },
    );
    ffi::emit(&Event { payload: Some(event::Payload::ViewChanged(convert::view(&view))) });
    ffi::emit(&Event { payload: Some(event::Payload::RosterChanged(convert::roster(&roster))) });

    // After the view, so that a pane surfaced by this very publish has somewhere to be
    // painted before it is told it is no longer waiting on anyone.
    for pane in &settled {
        announce_state(pane);
    }
}

/// Applies what a daemon just said to what Muster is holding open.
///
/// Separate from reporting it, and finished before reporting starts: reporting reaches the
/// shell, the shell reacts by dispatching, and a dispatch that arrived while this held the
/// session would deadlock against it on the same thread.
fn reconcile(daemon: &DaemonId) {
    {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.reconcile(daemon);
    }
    // A standing rule rather than a launch-time one, because the states that produce a
    // daemon with nothing on screen keep arriving: a workspace Muster asked for a moment ago
    // and is waiting on, a daemon that came back after a restart with its tabs, a tab closed
    // from another client while its daemon still holds others.
    //
    // Safe only while nothing closes a region deliberately - the day a user can put one away,
    // this would reopen it on the next thing the daemon said, and the rule needs to learn the
    // difference between empty and dismissed.
    open_remaining_regions();
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
            // Two questions, not one. Composition is reconciled when something it names may
            // have moved; the view and the roster are republished whenever they would read
            // differently - and a pane's name is in the roster without being anywhere
            // composition can see.
            if change.moves_structure() {
                reconcile(daemon);
            }
            if change.republishes() {
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
    }

    // The one change that is about an interval rather than a pane, and the only evidence
    // there will ever be that something happened while Muster was not listening. herdr's
    // counter is session-wide and reaches a client only in a snapshot, so nothing says
    // which panes moved or what they moved to - and after this line, nothing ever can.
    if let Change::AgentTransitionsMissed { expected, saw } = change {
        log::warn(
            "agent.transitions_missed",
            fields! {
                "daemon" => daemon.to_string(),
                "expected" => expected.to_string(),
                "saw" => saw.to_string(),
                "missed" => saw.saturating_sub(*expected).to_string(),
                "impact" => "the states shown are right, but an agent may have asked for a \
                             person while this window was disconnected and gone back to work \
                             since - so a request for attention was never routed",
                "check" => "the backend.stale record above this, for how long the gap was, and \
                            whether any pane on this daemon is waiting on somebody",
            },
        );
    }

    // Recorded before anything is announced, because it is what the announcement depends on:
    // whether this transition finished on a pane somebody was looking at is the difference
    // between `idle` and `done`.
    match change {
        Change::AgentStateChanged { pane, from, to } => {
            let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
            session.attention.observed(&PaneKey::new(daemon, pane), *from, *to);
        }
        // A pane that was already finished when this window arrived. Muster saw no transition
        // for it and the daemon did, so first sight takes the daemon's answer; everything
        // after it is Muster's own (`muster_core::attention`).
        Change::PaneAdded(pane) => {
            let key = PaneKey::new(daemon, pane);
            let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
            if let Some(backend) = session.agent_state(&key) {
                session.attention.first_seen(&key, backend);
            }
        }
        _ => {}
    }

    if let Some(pane) = change.announces_agent_state() {
        announce_state(&PaneKey::new(daemon, pane));
    }
}

/// Tells the shell what to paint for one pane's agent.
fn announce_state(pane: &PaneKey) {
    // Resolved before emitting, and with the lock let go in between. Emitting reaches the
    // shell, the shell reacts by dispatching, and a dispatch arriving while this held the
    // session would deadlock against it on the same thread.
    let Some(state) = presented(pane) else { return };
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneStateChanged(PaneStateChanged {
            daemon_id: pane.daemon.to_string(),
            pane_id: pane.pane.to_string(),
            state: state.as_str().to_string(),
        })),
    });
}

/// What the window should show for a pane, which is not always what the daemon said.
///
/// The mirror is read back rather than the change being taken at its word, because one of
/// the two changes that announce a pane carries no state at all - a pane that appears
/// already running is the case that needs this. For a transition the mirror was written
/// before this runs, so it holds exactly what the transition moved to.
///
/// Then `done` is decided here rather than accepted from the daemon, because the daemon
/// cannot see this window (`attention`).
fn presented(pane: &PaneKey) -> Option<AgentState> {
    let session = SESSION.lock().expect("a panicking sender poisoned the session");
    let backend = session.agent_state(pane)?;
    Some(session.attention.presented(pane, backend))
}

/// The window gained or lost the OS's focus.
///
/// The one thing about attention no daemon can tell the core and no core can observe. What
/// it changes is which finished agents are still waiting to be noticed, so only those panes
/// are re-announced - an agent-state change costs that change rather than a walk of every
/// pane (`architecture.md`, fast is a feature).
pub(crate) fn window_focused(focused: bool) {
    log::info("window.focus", fields! { "focused" => focused });
    let settled = {
        let mut session = SESSION.lock().expect("a panicking sender poisoned the session");
        session.attention.window_focused(focused)
    };
    for pane in &settled {
        announce_state(pane);
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
