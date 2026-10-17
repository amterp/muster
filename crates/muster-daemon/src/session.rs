//! Everything the daemon holds, and every control request's effect on it.
//!
//! One lock over all of it. A request takes the lock, changes what it changes, and emits its
//! events to every subscriber's queue while still holding it; the connection then queues the
//! answer, once any deferred work is done. That is the whole of the ordering the protocol promises: events
//! before the answer that names the last of them, and a subscription's snapshot before any event
//! after it. Nothing on a pane's output path takes this lock.
//!
//! Nothing holding this lock waits on a pane's lock, which attaching holds while it formats a
//! replay of a pane's whole history. Work on a pane's terminal that a request causes - hanging
//! it up, applying new settings - is left on the session as [`Deferred`] and done by the
//! [`Locked`] guard once the lock is let go, before the connection answers.
//!
//! Three requests let go of the lock part way, because each waits on something slow that would
//! otherwise stall every connection and every exit with it. A pane create waits for its process
//! to change directory and exec, and a directory can be on a hung mount: the create is checked
//! and its names reserved under the lock, the process starts without it, and the pane is
//! placed, its events emitted and its answer queued, under the lock again. `send_manifests` compiles
//! manifests and reads the override directory without it, and `pane.read` formats its page
//! without it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::ops::{Deref, DerefMut};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use prost::Message;
use proto::answer::Detail;

use crate::restart::{self, Restarting, Unrestartable};
use proto::event::Event as Payload;
use proto::request::Service;
use proto::{Outcome, pane_request, session_request, tab_request};

use crate::compaction::{Compaction, Pending};
use crate::control::Outbox;
use crate::daemon_log::DaemonLog;
use crate::data::Data;
use crate::detect::{self, Detecting, Loaded};
use crate::effects::{self, Report, Reported, Reports};
use crate::facts;
use crate::messages::{Doorbell, Messages, Peers};
use crate::pane::{Ended, Pane, PaneIo, Process, Turns, Watching};
use crate::persist::{self, Persister};
use crate::pty::{self, Grid, Launch};
use crate::screen::{self, Appearance, Screen, Settled};
use crate::server::Socket;
use crate::spawn;
use crate::tree::{self, Node, Resized};
use muster_detect::Manifests;

/// Where the daemon finds and is found.
#[derive(Debug)]
pub(crate) struct Places {
    /// Where a pane starts when nothing says where.
    pub(crate) home: PathBuf,
    /// A person's detection manifests, `~/.muster/agent-detection/`.
    pub(crate) overrides: Option<PathBuf>,
    pub(crate) reachable: spawn::Reachable,
    /// The daemon's own executable, which a `replace` starts unless told otherwise.
    pub(crate) executable: Option<PathBuf>,
    /// What the daemon gives its shells: the terminfo entry and the shell integration.
    pub(crate) data: Data,
    /// The daemon's own log, which a client can follow. None when logging is off.
    pub(crate) log: Option<Arc<DaemonLog>>,
}

/// What a daemon starts from besides its places: what it writes its state with, and the
/// settings the state it found held.
#[derive(Debug)]
pub(crate) struct Saved {
    pub(crate) persister: Arc<Persister>,
    pub(crate) settings: Option<proto::Settings>,
    /// Whether saved tabs are to come back ([`restore`]).
    pub(crate) restoring: bool,
}

/// Why the daemon exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A `stop` answered, or a signal: every pane is closed first.
    Asked,
    /// Every pane was handed to a new daemon, which serves the socket now: nothing is closed,
    /// and nothing of the socket's is removed.
    HandedOff,
}

/// What every thread of the daemon shares.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) session: Mutex<Session>,
    /// Messages between agents, under a lock of their own (see [`crate::messages`]).
    pub(crate) messages: Mutex<Messages>,
    /// Rings agents in panes once they can be rung.
    pub(crate) doorbell: Doorbell,
    /// Links to other machines' daemons, which groups span machines over.
    pub(crate) peers: Peers,
    /// The manifests panes are detected by, which the doorbell reads a prompt with too.
    pub(crate) detecting: Arc<Detecting>,
    /// Told when the daemon should exit, and why.
    pub(crate) stopping: Sender<Stop>,
    pub(crate) instance: u64,
    pub(crate) socket: Socket,
    /// The session's view of who joined messaging by what name, for a request to probe the
    /// inboxes it would name a pane after before the session's lock is taken.
    pub(crate) joined: crate::messages::Joined,
}

impl Shared {
    pub(crate) fn new(
        instance: u64,
        stopping: Sender<Stop>,
        inherited: Vec<(OsString, OsString)>,
        places: Places,
        saved: Saved,
        socket: Socket,
    ) -> Arc<Shared> {
        let Places { home, overrides, reachable, executable, data, log } = places;
        let Saved { persister, settings, restoring } = saved;
        let built = Arc::new_cyclic(|shared: &Weak<Shared>| {
            let (reports, received) = Reports::channel();
            let publishing = shared.clone();
            let publisher = std::thread::Builder::new()
                .name("publish".to_string())
                .spawn(move || effects::publish(&received, &publishing));
            if let Err(error) = publisher {
                log::error(
                    "daemon.publisher.no_thread",
                    fields! {
                        "error" => error,
                        "impact" => "no pane's title, directory, agent state, bell or \
                                     notification will be published by this daemon",
                        "check" => "whether the daemon is out of threads",
                    },
                );
            }
            persister.start(shared.clone());
            let shared = shared.clone();
            let ended: Ended = Arc::new(move |serial, status| {
                if let Some(shared) = shared.upgrade() {
                    shared.lock().ended(serial, status);
                }
            });
            let mut settings = settings.unwrap_or_default();
            settings.shell.get_or_insert_default();
            let settled = Arc::new(Settled {
                generation: 0,
                appearance: Appearance::of(&settings),
                scrollback: scrollback(&settings),
                scroll_multiplier: settings.scroll_multiplier.unwrap_or(1.0),
            });
            let detecting = Detecting::start(overrides);
            let messages = Messages::load(&socket.path);
            let human = messages.human();
            let joined = messages.joined();
            Shared {
                messages: Mutex::new(messages),
                doorbell: Doorbell::default(),
                peers: Peers::beside(&socket.path),
                detecting: Arc::clone(&detecting),
                joined: Arc::clone(&joined),
                session: Mutex::new(Session {
                    instance,
                    seq: 0,
                    tabs: Vec::new(),
                    panes: Vec::new(),
                    settings,
                    settled,
                    deferred: Vec::new(),
                    orphans: Vec::new(),
                    detecting,
                    app_manifests: Vec::new(),
                    manifest_loads: 0,
                    manifests_adopted: 0,
                    subscribers: Vec::new(),
                    attending: HashSet::new(),
                    human: human
                        .into_iter()
                        .map(|notice| (notice.group.clone(), (0, notice)))
                        .collect(),
                    joined,
                    probed: Probed::default(),
                    inherited,
                    home,
                    data,
                    reachable,
                    executable,
                    next_serial: 0,
                    ended,
                    reports,
                    host: effects::host_name(),
                    reserved: HashSet::new(),
                    naming: HashMap::new(),
                    persister,
                    log,
                    stopping: false,
                    restoring,
                    restored_from_file: false,
                    replacing: Replacing::No,
                    stop_deferred: false,
                }),
                stopping,
                instance,
                socket,
            }
        });
        // Not from inside `new_cyclic`: until it returns, the weak handle cannot be upgraded, and
        // the watcher would read that as the daemon having stopped and end for good.
        let (stopping, reloading) = (Arc::downgrade(&built), Arc::downgrade(&built));
        detect::watch_overrides(
            Arc::clone(&built.detecting),
            move || stopping.upgrade().is_none_or(|shared| shared.lock().stopping),
            move || {
                if let Some(shared) = reloading.upgrade() {
                    shared.reload_overrides();
                }
            },
        );
        built
    }

    /// Starts the doorbell, once the panes this daemon begins with are in: its first look rings
    /// the wakes it loaded, and drops those whose pane it cannot find and no restore under way
    /// may bring back.
    pub(crate) fn start_doorbell(self: &Arc<Self>) {
        self.doorbell.start(Arc::downgrade(self));
    }

    pub(crate) fn lock(&self) -> Locked<'_> {
        Locked {
            session: Some(poison::lock(&self.session, "daemon.session")),
            doorbell: &self.doorbell,
        }
    }

    pub(crate) fn messages(&self) -> MutexGuard<'_, Messages> {
        poison::lock(&self.messages, "daemon.messages")
    }

    /// Points the link beside the socket at this daemon, and tells panes started from now the
    /// link. The link is written with the session unlocked.
    pub(crate) fn point_link(&self) {
        let (socket, executable) = {
            let session = self.lock();
            (session.reachable.socket.clone(), session.executable.clone())
        };
        let Some(executable) = executable else { return };
        let daemon = crate::server::point_link(&socket, &executable);
        self.lock().reachable.daemon = Some(daemon);
    }

    /// Compiles the manifests again with the app's last ones, after a person's override directory
    /// changed: the same path the app's own send takes, so only the panes whose agent is detected
    /// differently now start over.
    pub(crate) fn reload_overrides(&self) {
        let manifests = self
            .lock()
            .app_manifests
            .iter()
            .map(|(agent, toml)| proto::Manifest { agent: agent.clone(), toml: toml.clone() })
            .collect();
        self.adopt_manifests(manifests);
    }

    /// Puts in use the manifests the app sent the daemon this one replaced.
    pub(crate) fn adopt_manifests(&self, manifests: Vec<proto::Manifest>) {
        let sent = proto::SendManifests { engine: 0, manifests };
        let Handled::Manifests(loading) = self.lock().send_manifests(sent) else { return };
        let loaded = loading.load();
        self.lock().manifests_loaded(*loading, loaded);
    }
}

impl Places {
    /// Where this process's daemon finds everything, besides the data directory and the log it
    /// was given.
    pub(crate) fn of_this_process(
        socket: &Path,
        data: Data,
        log: Option<Arc<DaemonLog>>,
    ) -> Places {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let overrides = muster_daemon_proto::install::muster_home(|name| std::env::var(name).ok())
            .map(|muster_home| muster_home.join("agent-detection"));
        let executable = std::env::current_exe().ok();
        let reachable =
            spawn::Reachable { daemon: executable.clone(), socket: socket.to_path_buf() };
        Places { home, overrides, reachable, executable, data, log }
    }
}

/// The session, locked. Letting go of it does whatever work on panes' terminals the holder
/// left deferred, once the lock is released.
pub(crate) struct Locked<'a> {
    session: Option<MutexGuard<'a, Session>>,
    /// Woken once the lock is let go when a maker closed with helpers to tell the person of.
    doorbell: &'a Doorbell,
}

impl Deref for Locked<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        self.session.as_ref().expect("held until dropped")
    }
}

impl DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        self.session.as_mut().expect("held until dropped")
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let Some(mut session) = self.session.take() else { return };
        let deferred = std::mem::take(&mut session.deferred);
        let orphaned = !session.orphans.is_empty();
        drop(session);
        for work in deferred {
            work.run();
        }
        if orphaned {
            self.doorbell.nudge();
        }
    }
}

/// A maker whose pane closed with helpers still open on this daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Orphaned {
    pub(crate) maker: String,
    /// What the maker was called as it closed.
    pub(crate) called: String,
    /// Each helper's pane, and what it is called.
    pub(crate) helpers: Vec<(String, String)>,
}

/// Work on a pane's terminal that a request caused, done once the session is unlocked.
enum Deferred {
    HangUp(Box<Pane>, proto::DetachReason),
    Settle(Arc<PaneIo>, Arc<Settled>),
}

impl Deferred {
    fn run(self) {
        match self {
            Deferred::HangUp(pane, reason) => pane.hang_up(reason),
            Deferred::Settle(io, settled) => io.settle(&settled),
        }
    }
}

#[expect(clippy::struct_excessive_bools, reason = "independent facts about one daemon run")]
pub(crate) struct Session {
    instance: u64,
    /// The last event's sequence number.
    seq: u64,
    /// In the order they were opened.
    tabs: Vec<Tab>,
    /// In the order they were opened. Every pane is in exactly one tab's tree.
    panes: Vec<Pane>,
    settings: proto::Settings,
    /// What `settings` means for a pane's terminal, numbered: every pane has it or is about to.
    settled: Arc<Settled>,
    /// Work on panes' terminals to do once the lock is let go ([`Locked`]).
    deferred: Vec<Deferred>,
    /// Makers that closed with helpers still open here, for the doorbell's thread to tell the
    /// person of ([`Session::take_orphans`]).
    orphans: Vec<Orphaned>,
    /// The manifests every pane's agent is detected by, which each pane's reader reads.
    detecting: Arc<Detecting>,
    /// The manifests the app last sent, by name, which every reload layers in.
    app_manifests: Vec<(String, String)>,
    /// How many `send_manifests` have been asked for, and which of them last put its manifests
    /// in use.
    manifest_loads: u64,
    manifests_adopted: u64,
    subscribers: Vec<Outbox>,
    /// The subscribers that are windows telling the human what waits for them, by connection.
    attending: HashSet<u64>,
    /// What waits for the human, per group, as last told to the windows, with the order the
    /// message service decided it in: two posts decided one after the other can reach here the
    /// other way round, and the older must not overwrite the newer.
    human: BTreeMap<String, (u64, proto::msg_answer::Notice)>,
    /// The agents that joined messaging by names of their own, which no pane is drawn, nor given
    /// while they are live, as messaging keeps them.
    joined: crate::messages::Joined,
    /// What the request being handled found at the inboxes of the agents it would name a pane
    /// after, set around [`Session::handle`] by the connection that took the lock.
    pub(crate) probed: Probed,
    /// The daemon's own environment, which every pane's starts from.
    inherited: Vec<(OsString, OsString)>,
    /// Where a pane starts when nothing says where.
    home: PathBuf,
    /// What the daemon gives its shells: the terminfo entry and the shell integration.
    data: Data,
    /// How a pane's programs reach this daemon, which every pane's environment says.
    reachable: spawn::Reachable,
    executable: Option<PathBuf>,
    next_serial: u64,
    ended: Ended,
    /// Where panes send what their programs asked for, for the publisher to apply here.
    reports: Reports,
    /// This machine's name, which OSC 7 URLs from a shell here carry.
    host: String,
    /// Pane and tab names a create has claimed while its process starts, so a second create
    /// cannot claim them too.
    reserved: HashSet<String>,
    /// Pane names a create or a restore has claimed while its process starts, and the pane each
    /// is for, so that no other pane can be called the same meanwhile.
    naming: HashMap<String, String>,
    /// Writes down what a restart needs, whenever it changes.
    persister: Arc<Persister>,
    /// The daemon's own log, which a connection can follow. None when logging is off.
    log: Option<Arc<DaemonLog>>,
    /// Set once the daemon has begun to stop, after which no pane starts.
    stopping: bool,
    /// Set while the tabs a previous run saved are coming back.
    restoring: bool,
    /// Whether a pane this run holds came back from the file a previous run wrote, so that
    /// every process in it was started by this run ([`proto::Snapshot::restored_from_file`]).
    restored_from_file: bool,
    replacing: Replacing,
    /// Set when a stop was asked for while a handoff ran, to be carried out if it fails.
    stop_deferred: bool,
}

/// What a stop asked for does, given where a handoff stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopping {
    /// Every pane has been closed; the daemon goes on to stop.
    Now,
    /// A handoff is under way, and the stop waits for it to end.
    Deferred,
    /// Another daemon serves every pane, and this one exits touching none of them.
    HandedOff,
}

/// Where the daemon stands in handing its panes to a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacing {
    No,
    /// Only a request that changes nothing is served.
    Underway,
    /// The new daemon serves: nothing here acts on a pane again.
    HandedOff,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("seq", &self.seq)
            .field("tabs", &self.tabs.len())
            .field("panes", &self.panes.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct Tab {
    name: String,
    label: proto::Label,
    root: Node,
    zoomed: Option<String>,
    /// No view shows it (`proto::Tab::hidden`). Set only when the tab is made.
    hidden: bool,
}

impl Tab {
    fn record(&self) -> proto::Tab {
        proto::Tab {
            tab: self.name.clone(),
            label: Some(self.label.clone()),
            root: Some(node_record(&self.root)),
            zoomed: self.zoomed.clone(),
            hidden: self.hidden,
        }
    }
}

/// What a request comes to, before the connection gives it an id and a sequence number.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) outcome: Outcome,
    pub(crate) reason: String,
    pub(crate) detail: Option<Box<Detail>>,
}

impl Reply {
    pub(crate) fn done() -> Reply {
        Reply { outcome: Outcome::Done, reason: String::new(), detail: None }
    }

    fn already() -> Reply {
        Reply { outcome: Outcome::AlreadySo, reason: String::new(), detail: None }
    }

    fn not_there(what: impl Into<String>) -> Reply {
        Reply { outcome: Outcome::NotThere, reason: what.into(), detail: None }
    }

    pub(crate) fn refused(why: impl Into<String>) -> Reply {
        Reply { outcome: Outcome::Refused, reason: why.into(), detail: None }
    }

    pub(crate) fn unsupported() -> Reply {
        Reply::refused(
            "this daemon does not know that request: it is empty, or newer than the protocol \
             this daemon speaks",
        )
    }
}

/// What handling a request came to.
#[derive(Debug)]
pub(crate) enum Handled {
    Reply(Reply),
    /// A pane to start with the session unlocked, then finish with [`Session::started`].
    Start(Box<Starting>),
    /// A pane's text to read with the session unlocked, since a long page takes a while to
    /// format. It holds the pane's lock a batch of rows at a time.
    Read(Box<Reading>),
    /// Manifests to compile with the session unlocked, then put in use with
    /// [`Session::manifests_loaded`].
    Manifests(Box<Loading>),
    /// An answer carrying a snapshot, queued before the session is let go: an event queued
    /// between the two would reach a subscriber ahead of a snapshot older than it.
    Snapshot(Reply),
    /// A handoff to run with the session unlocked ([`crate::handoff::hand_over`]).
    Replace(Box<Replacement>),
    /// A restart to run with the session unlocked, and answered when it is done.
    Restart(Box<Restarting>),
}

/// A `replace` that has been checked: the daemon to hand every pane to.
#[derive(Debug)]
pub(crate) struct Replacement {
    pub(crate) program: PathBuf,
    pub(crate) data: Option<PathBuf>,
}

/// What a handoff sends, taken out of the session in one hold of its lock so that the state and
/// the panes agree.
pub(crate) struct Handing {
    pub(crate) state: persist::State,
    pub(crate) app_manifests: Vec<(String, String)>,
    pub(crate) panes: Vec<HandedPane>,
    pub(crate) persister: Arc<Persister>,
    pub(crate) log: Option<Arc<DaemonLog>>,
}

/// Where a pane handed over had got to with its agent, beyond what its record says.
#[derive(Clone, Copy)]
pub(crate) struct Resuming<'a> {
    pub(crate) detection: Option<&'a proto::handoff::Detection>,
    pub(crate) turns: Turns,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) resume: Option<&'a persist::Resume>,
    /// The number of the oldest row the replay holds.
    pub(crate) oldest_row: u64,
}

pub(crate) struct HandedPane {
    pub(crate) record: proto::Pane,
    pub(crate) process: Option<i32>,
    pub(crate) io: Arc<PaneIo>,
    pub(crate) turns: Turns,
    pub(crate) session_id: Option<String>,
}

/// Why a daemon being replaced refuses a request that changes anything. A caller that means to
/// ask again once the new daemon serves knows the refusal by it.
pub(crate) const HANDING_OVER: &str =
    "the daemon is handing its panes to a new one; ask the new one once it serves";

/// Whether a request changes anything, which a daemon being replaced refuses: what it holds
/// has been, or is being, handed over as it stands.
fn changes_anything(service: &Service) -> bool {
    use pane_request::Request as P;
    use session_request::Request as S;
    !matches!(
        service,
        Service::Session(proto::SessionRequest {
            request: Some(S::Snapshot(_) | S::Subscribe(_) | S::FollowLog(_) | S::ReadGrids(_))
        }) | Service::Pane(proto::PaneRequest { request: Some(P::Read(_)) })
    )
}

/// A `send_manifests` whose manifests have yet to be compiled.
#[derive(Debug)]
pub(crate) struct Loading {
    detecting: Arc<Detecting>,
    app: Vec<(String, String)>,
    /// Which `send_manifests` this is, so one that finishes loading after a later one does not
    /// replace what the later one put in use.
    load: u64,
}

impl Loading {
    pub(crate) fn load(&self) -> Loaded {
        self.detecting.load(&self.app)
    }
}

/// A `pane.read` whose pane has been found.
#[derive(Debug)]
pub(crate) struct Reading {
    io: Arc<PaneIo>,
    first_row: u64,
    rows: u32,
    /// The last rows asked for instead, when not zero.
    last: u32,
    /// What the pane's agent printed in its last turn asked for instead.
    turn: bool,
    /// The pane's agent, and what reads its prompt, when what the prompt holds was asked for too.
    prompt: Option<(String, Arc<Detecting>)>,
}

impl Reading {
    pub(crate) fn read(&self) -> Reply {
        let rows = |first, count| self.io.screen().rows(first, count);
        let text = if self.turn {
            let Some(start) = self.io.turn_start() else {
                return Reply::refused(
                    "no turn has started in this pane since this daemon began watching it, so \
                     there is no last turn to read; read its newest rows with --rows instead",
                );
            };
            proto::PaneText {
                turn: Some(start.row),
                turn_moved: start.moved,
                ..screen::since(start.row, screen::PAGE_BYTES, rows)
            }
        } else if self.last > 0 && self.last <= screen::HELD_TAIL {
            let held = self.io.screen();
            screen::last_page(self.last, screen::PAGE_BYTES, |first, count| held.rows(first, count))
        } else if self.last > 0 {
            screen::last_page(self.last, screen::PAGE_BYTES, rows)
        } else {
            screen::page(self.first_row, self.rows, screen::PAGE_BYTES, rows)
        };
        let prompt = self.prompt.as_ref().and_then(|(agent, detecting)| {
            crate::messages::prompt_holds(&self.io, agent, detecting)
        });
        let text = proto::PaneText { prompt, ..text };
        Reply { detail: Some(Box::new(Detail::Text(text))), ..Reply::done() }
    }
}

/// A process started for a pane, or why it did not.
type Started = std::io::Result<(OwnedFd, Child)>;

/// What a pane's process starts as: its argv and environment.
type Launched = (Vec<String>, Vec<(OsString, OsString)>);

/// A saved tab whose names are reserved, and whose panes' shells have yet to start.
#[derive(Debug)]
struct Restoring {
    tab: persist::Tab,
    panes: Vec<RestoringPane>,
    /// Where a pane starts when its saved directory is gone.
    home: PathBuf,
}

#[derive(Debug)]
struct RestoringPane {
    saved: persist::Pane,
    /// What starts its agent's session again, when it starts with that rather than a shell.
    resume: Option<persist::Resume>,
    /// The configured shell, running the resume when there is one.
    launch: Launched,
    /// The configured shell running nothing, for a resume whose directory is gone.
    plain: Launched,
    /// The default shell, for when the configured one will not start.
    fallback: Launched,
}

/// A saved pane's shell once it has started, or failed to: where, and which program.
#[derive(Debug)]
struct Restarted {
    cwd: PathBuf,
    program: String,
    started: Started,
    /// Whether what started is the pane's agent session resumed, rather than a shell.
    resumed: bool,
}

impl Restoring {
    /// Starts each pane's shell in its saved directory, or at home when that is gone. A shell
    /// and never the command a pane was made with: an agent started afresh in every pane is not
    /// what anybody asked for. A pane whose agent reported its session starts that session
    /// again instead, through the shell it drops back to when the agent exits.
    ///
    /// A shell that will not start there - a configured shell since uninstalled, a directory it
    /// may not enter - is tried again as the default shell, in the same directory and then at
    /// home, so a setting that went bad between runs costs neither the panes nor where they
    /// start, and a directory that did costs only where they start.
    fn start(&self, directories: &HashMap<PathBuf, Option<bool>>) -> Vec<Restarted> {
        self.panes
            .iter()
            .map(|pane| {
                let saved = &pane.saved.cwd;
                let cwd = match directories.get(saved).copied().flatten() {
                    Some(true) => saved.clone(),
                    Some(false) => {
                        log::warn(
                            "daemon.state.cwd_gone",
                            fields! {
                                "pane" => pane.saved.name,
                                "cwd" => saved.display(),
                                "impact" => "the pane's shell starts in the home directory instead",
                                "check" => "whether the directory was removed or its mount is gone",
                            },
                        );
                        self.home.clone()
                    }
                    None => {
                        log::warn(
                            "daemon.state.cwd_unanswered",
                            fields! {
                                "pane" => pane.saved.name,
                                "cwd" => saved.display(),
                                "seconds" => DIRECTORY_PATIENCE.as_secs(),
                                "impact" => "the pane's shell starts in the home directory \
                                             instead, and a thread still waiting on the directory \
                                             is left until it answers",
                                "check" => "whether the directory is on a network mount that has \
                                            hung",
                            },
                        );
                        self.home.clone()
                    }
                };
                // A session resumed anywhere but where it ran finds no conversation to resume, so a
                // pane whose directory is gone comes back as a shell, and its agent as stopped.
                let resumes = pane.resume.is_some() && cwd == *saved;
                if pane.resume.is_some() && !resumes {
                    log::warn(
                        "daemon.state.resume_skipped",
                        fields! {
                            "pane" => pane.saved.name,
                            "impact" => "the pane comes back as a shell rather than running its \
                                         agent's session, which would not find its conversation \
                                         from another directory",
                            "check" => "the daemon.state.cwd line above says why the directory \
                                        could not be used; restore it and start the session \
                                        again by hand",
                        },
                    );
                }
                let launch = if resumes { &pane.launch } else { &pane.plain };
                let mut first = start_launched(launch, &cwd, pane.saved.grid);
                first.resumed = resumes;
                let Err(error) = first.started else { return first };
                log::warn(
                    "daemon.state.fallback",
                    fields! {
                        "pane" => pane.saved.name,
                        "program" => first.program,
                        "cwd" => cwd.display(),
                        "error" => error,
                        "impact" => "the default shell is tried instead, in this directory and \
                                     then at home",
                        "check" => "whether the configured shell still exists, and whether the \
                                    directory can be entered",
                    },
                );
                let fallback = start_launched(&pane.fallback, &cwd, pane.saved.grid);
                if fallback.started.is_ok() || cwd == self.home {
                    return fallback;
                }
                start_launched(&pane.fallback, &self.home, pane.saved.grid)
            })
            .collect()
    }
}

/// The names of the flags among `arguments`, space-separated: `--model --settings`. A flag's
/// value, whether after `=` or as the next word, is left out.
fn flag_names(arguments: &[String]) -> String {
    arguments
        .iter()
        .filter(|word| word.starts_with('-'))
        .map(|flag| flag.split('=').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How long restoring waits to learn whether a pane's saved directory is there. A directory on a
/// mount that has hung never answers, and would otherwise hold every tab after it back.
const DIRECTORY_PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

/// The longest focus typed after an agent's compact command: a line for the summary to keep,
/// not a brief.
const FOCUS_BYTES: usize = 512;

/// Whether each of `paths` is a directory, asked by `probe` on a thread per path, all at once
/// and given `within` between them: N panes on a hung mount cost `within`, not N times it. None
/// for a path that did not answer in time; its thread is left waiting, one per such directory.
fn probe_directories(
    paths: impl IntoIterator<Item = PathBuf>,
    within: std::time::Duration,
    probe: fn(&Path) -> bool,
) -> HashMap<PathBuf, Option<bool>> {
    let deadline = std::time::Instant::now() + within;
    let (answer, answered) = std::sync::mpsc::channel();
    let mut found = HashMap::new();
    let mut waiting = 0;
    for path in paths {
        if found.contains_key(&path) {
            continue;
        }
        found.insert(path.clone(), None);
        let answer = answer.clone();
        let spawned =
            std::thread::Builder::new().name("probe directory".to_string()).spawn(move || {
                let is_dir = probe(&path);
                let _ = answer.send((path, is_dir));
            });
        waiting += usize::from(spawned.is_ok());
    }
    while waiting > 0 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok((path, is_dir)) = answered.recv_timeout(left) else { break };
        found.insert(path, Some(is_dir));
        waiting -= 1;
    }
    found
}

fn start_launched((argv, environment): &Launched, cwd: &Path, grid: Grid) -> Restarted {
    let launch = Launch { argv, environment, cwd, grid };
    Restarted {
        cwd: cwd.to_path_buf(),
        program: argv[0].clone(),
        started: pty::start(&launch),
        resumed: false,
    }
}

/// What a restore could not bring back, as [`proto::Restored`] names it.
#[derive(Debug, Default)]
struct Lost {
    tabs: Vec<String>,
    /// Every saved pane not brought back, a lost tab's among them.
    panes: Vec<String>,
}

impl Lost {
    fn is_empty(&self) -> bool {
        self.tabs.is_empty() && self.panes.is_empty()
    }
}

/// Brings back the tabs a previous run saved, one at a time, each pane's shell starting with
/// the session unlocked. Only then may the persister write: until every saved tab is back, the
/// session holds less than the file, and a write would lose the difference. What could not be
/// brought back is kept in a copy of the file, since the next write leaves it out.
pub(crate) fn restore(shared: &Shared, state: persist::State) {
    let started = std::time::Instant::now();
    let saved: HashMap<String, persist::Pane> =
        state.panes.into_iter().map(|pane| (pane.name.clone(), pane)).collect();
    let names: Vec<(String, Vec<String>)> = state
        .tabs
        .iter()
        .map(|tab| (tab.name.clone(), tab.root.panes().into_iter().map(str::to_string).collect()))
        .collect();
    // Every directory asked about up front, together: a hung mount then costs the restore one
    // wait rather than one per pane in it.
    let directories = probe_directories(
        saved.values().map(|pane| pane.cwd.clone()),
        DIRECTORY_PATIENCE,
        Path::is_dir,
    );
    let mut lost = Lost::default();
    let mut done = 0;
    let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for tab in state.tabs {
            let restoring = shared.lock().prepare_restore(tab, &saved, &mut lost);
            if let Some(restoring) = restoring {
                let started = restoring.start(&directories);
                shared.lock().restored(restoring, started, &mut lost);
            }
            done += 1;
        }
    }));
    let failed = finished.err().map(|panic| {
        let session = shared.lock();
        for (tab, panes) in &names[done..] {
            if session.tab_index(tab).is_none() && !lost.tabs.contains(tab) {
                lost.tabs.push(tab.clone());
            }
            let gone = |pane: &&String| session.pane_index(pane).is_none();
            for pane in panes.iter().filter(gone) {
                if !lost.panes.contains(pane) {
                    lost.panes.push(pane.clone());
                }
            }
        }
        panic
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no message".to_string())
    });
    {
        // Taken apart from the session's lock, which is never held with it. Wakes for panes
        // brought back waited for them, and wait now for their agents.
        let wakes = shared.messages().waiting();
        let session = shared.lock();
        log::info(
            "daemon.state.restored",
            fields! {
                "tabs" => session.tabs.len(),
                "saved_tabs" => names.len(),
                "panes" => session.panes.len(),
                "saved_panes" => saved.len(),
                "wakes" => wakes,
                "ms" => started.elapsed().as_millis(),
            },
        );
    }
    finish_restore(shared, lost, failed.as_deref());
}

/// Ends a restore, however it went: a client waits for [`proto::Restored`] before deciding
/// a saved tab is gone, so it is sent even when restoring failed. `failed` is a panic's message.
fn finish_restore(shared: &Shared, lost: Lost, failed: Option<&str>) {
    if let Some(panic) = failed {
        log::error(
            "daemon.state.restore_failed",
            fields! {
                "panic" => panic,
                "impact" => "the saved tabs are not all back, and nothing is saved for the rest of \
                             this daemon's run; the file keeps what the last run saved",
                "check" => "this is a bug: the panic, here and on the daemon's stderr, says where \
                            restoring stopped",
            },
        );
    }
    let persister = {
        let mut session = shared.lock();
        if session.stopping {
            session.restoring = false;
            return;
        }
        Arc::clone(&session.persister)
    };
    // Outside the lock, as every other disk operation is: a hung disk must not stall every
    // connection.
    let saving =
        failed.is_none() && (lost.is_empty() || keep_what_was_lost(persister.path(), &lost));
    let mut session = shared.lock();
    session.restoring = false;
    if session.stopping {
        return;
    }
    let Lost { tabs, panes } = lost;
    session.emit(Payload::Restored(proto::Restored {
        lost_tabs: tabs,
        lost_panes: panes,
        saving_stopped: !saving,
    }));
    if saving {
        persister.arm();
    } else {
        persister.off();
    }
}

/// Copies the state file aside before the persister's first write leaves out the saved tabs
/// and panes that did not come back. False when the copy failed, and nothing may be written
/// this run: the file is then the only record of them.
fn keep_what_was_lost(path: &Path, lost: &Lost) -> bool {
    match persist::keep_aside(path) {
        Ok(kept) => {
            log::warn(
                "daemon.state.unrestored",
                fields! {
                    "lost_tabs" => lost.tabs.join(" "),
                    "lost_panes" => lost.panes.join(" "),
                    "kept" => kept.display(),
                    "impact" => "these saved tabs and panes did not come back, and the next write \
                                 leaves them out of the state file; the file as it was is kept \
                                 aside",
                    "check" => "the daemon.state.* and daemon.pane.not_started records before \
                                this one say why each did not come back",
                },
            );
            true
        }
        Err(error) => {
            log::error(
                "daemon.state.unrestored",
                fields! {
                    "lost_tabs" => lost.tabs.join(" "),
                    "lost_panes" => lost.panes.join(" "),
                    "error" => error,
                    "impact" => "these saved tabs and panes did not come back, and the file \
                                 holding them could not be copied aside, so nothing is saved for \
                                 the rest of this daemon's run",
                    "check" => "the permissions on the state file's directory, and free space",
                },
            );
            false
        }
    }
}

/// A create that has been checked, with its names reserved, and whose process has yet to start.
#[derive(Debug)]
pub(crate) struct Starting {
    pane: String,
    label: Option<String>,
    /// The word drawn for it, and claimed in `naming` with the name it was given, if any.
    word: String,
    command: Option<String>,
    quiet: bool,
    maker: Option<String>,
    target: Target,
    grid: Grid,
    cwd: PathBuf,
    argv: Vec<String>,
    environment: Vec<(OsString, OsString)>,
}

impl Starting {
    /// Opens the pane's PTY and starts its process. Waits for the process to change directory
    /// and exec, which is why the session is not locked while this runs.
    pub(crate) fn start(&self) -> std::io::Result<(OwnedFd, Child)> {
        let launch = Launch {
            argv: &self.argv,
            environment: &self.environment,
            cwd: &self.cwd,
            grid: self.grid,
        };
        pty::start(&launch)
    }
}

/// Where a create or a move puts a pane, once checked against what is here.
#[derive(Debug)]
enum Target {
    Beside {
        pane: String,
        side: tree::Side,
        ratio: f32,
    },
    NewTab {
        name: String,
        label: proto::Label,
        hidden: bool,
    },
    /// Among `maker`'s helpers, which is decided as the pane is placed: two helpers started at
    /// once would otherwise each find no tab of helpers and make one. `name` and `label` are for
    /// the hidden tab made when no tab holds any of them yet.
    Helpers {
        maker: String,
        name: String,
        label: proto::Label,
    },
}

impl Target {
    /// The name of the tab this may make, reserved from when it is checked until it is placed.
    fn new_tab(&self) -> Option<&str> {
        match self {
            Target::NewTab { name, .. } | Target::Helpers { name, .. } => Some(name),
            Target::Beside { .. } => None,
        }
    }
}

impl Session {
    /// The last event's sequence number, which an answer carries.
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn handle(&mut self, service: Service, asker: &Outbox) -> Handled {
        use pane_request::Request as P;
        use session_request::Request as S;
        use tab_request::Request as T;
        if self.replacing != Replacing::No && changes_anything(&service) {
            return Handled::Reply(Reply::refused(HANDING_OVER));
        }
        let reply = match service {
            Service::Session(proto::SessionRequest { request: Some(request) }) => match request {
                S::Snapshot(_) => {
                    return Handled::Snapshot(Reply {
                        detail: Some(Box::new(Detail::Snapshot(self.snapshot()))),
                        ..Reply::done()
                    });
                }
                S::Subscribe(subscribe) => {
                    return Handled::Snapshot(self.subscribe(asker, subscribe.attends));
                }
                S::SetShell(set) => self.set_shell(set),
                S::SetScrollback(set) => self.set_scrollback(set),
                S::SetPalette(set) => self.set_palette(set),
                S::SendManifests(manifests) => return self.send_manifests(manifests),
                S::SetClipboardWrite(set) => self.set_clipboard_write(set),
                S::SetCursor(set) => self.set_cursor(set),
                S::SetScrollMultiplier(set) => self.set_scroll_multiplier(set.multiplier),
                S::SetNameSessions(set) => self.set_name_sessions(set.name),
                S::SetResumeAgents(set) => self.set_resume_agents(set.resume),
                S::SetHumanName(set) => self.set_human_name(set.name),
                S::SetCompactAt(set) => self.set_compact_at(set.percent),
                S::ReadGrids(_) => self.grids(),
                S::FollowLog(follow) => self.follow_log(asker, follow.after),
                S::Replace(replace) => return self.replace(replace),
                S::Stop(_) => {
                    self.close_everything();
                    Reply::done()
                }
            },
            Service::Tab(proto::TabRequest { request: Some(request) }) => match request {
                T::Close(close) => self.close_tab(&close.tab),
                T::Rename(rename) => self.rename_tab(rename),
                T::SetSplitRatio(set) => self.set_split_ratio(&set),
                T::SetHidden(set) => self.set_tab_hidden(set),
            },
            Service::Pane(proto::PaneRequest { request: Some(request) }) => match request {
                P::Create(create) => return self.create(create),
                P::Close(close) => self.close_pane(&close.pane),
                P::Resize(resize) => self.resize(&resize),
                P::Zoom(zoom) => self.zoom(&zoom),
                P::Swap(swap) => self.swap(&swap),
                P::Move(moved) => self.move_pane(moved),
                P::Rename(rename) => self.rename_pane(rename),
                P::Report(report) => self.report(report),
                P::Seen(seen) => self.seen(&seen.panes),
                P::Compact(compact) => self.compact(&compact),
                P::Quiet(quiet) => self.quiet(&quiet),
                P::MakerClosed(closed) => self.maker_closed(&closed),
                P::Restart(restart) => match self.restart(&restart) {
                    Ok(restarting) => return Handled::Restart(Box::new(restarting)),
                    Err(refused) => refused,
                },
                P::Read(read) => match self.pane_index(&read.pane) {
                    None => Reply::not_there(format!("no pane {} on this daemon", read.pane)),
                    Some(index) => {
                        return Handled::Read(Box::new(Reading {
                            io: Arc::clone(&self.panes[index].io),
                            first_row: read.first_row,
                            rows: read.rows,
                            last: read.last,
                            turn: read.turn,
                            prompt: read
                                .prompt
                                .then(|| self.panes[index].record.agent.clone())
                                .flatten()
                                .map(|agent| (agent, Arc::clone(&self.detecting))),
                        }));
                    }
                },
            },
            _ => Reply::unsupported(),
        };
        Handled::Reply(reply)
    }

    /// Stops listening to a connection that has gone.
    pub(crate) fn unsubscribe(&mut self, connection: u64) {
        self.subscribers.retain(|subscriber| subscriber.id != connection);
        self.attending.remove(&connection);
        if let Some(log) = &self.log {
            log.unfollow(connection);
        }
    }

    /// How big every pane's terminal is now, by name.
    fn grids(&self) -> Reply {
        let panes = self
            .panes
            .iter()
            .map(|pane| {
                let grid = pane.io.grid();
                let size = proto::Grid {
                    cols: u32::from(grid.cols),
                    rows: u32::from(grid.rows),
                    width_px: u32::from(grid.width_px),
                    height_px: u32::from(grid.height_px),
                };
                (pane.record.pane.clone(), size)
            })
            .collect();
        Reply { detail: Some(Box::new(Detail::Grids(proto::Grids { panes }))), ..Reply::done() }
    }

    /// Hands a connection the daemon's recent log, and every record after it.
    fn follow_log(&self, asker: &Outbox, after: Option<u64>) -> Reply {
        let Some(log) = &self.log else {
            return Reply::refused("this daemon's log is off: it was started with MUSTER_LOG=0");
        };
        let (oldest, newest) = log.follow(asker, after);
        Reply {
            detail: Some(Box::new(Detail::Followed(proto::LogFollowed { oldest, newest }))),
            ..Reply::done()
        }
    }

    /// Closes every tab, and so every pane, in the order they were opened, and starts no more.
    /// What the daemon held before is what it leaves written down: a daemon that stops is not
    /// one told to forget its tabs.
    pub(crate) fn close_everything(&mut self) {
        if !self.stopping {
            self.persister.stopping(self.persisted());
        }
        self.stopping = true;
        while let Some(tab) = self.tabs.first() {
            let name = tab.name.clone();
            self.close_tab(&name);
        }
    }

    fn snapshot(&self) -> proto::Snapshot {
        proto::Snapshot {
            seq: self.seq,
            instance: self.instance,
            tabs: self.tabs.iter().map(Tab::record).collect(),
            panes: self.panes.iter().map(|pane| pane.record.clone()).collect(),
            settings: Some(self.settings.clone()),
            restoring: self.restoring,
            restored_from_file: self.restored_from_file,
            human: self
                .human
                .values()
                .filter(|(_, notice)| notice.count > 0 || notice.member)
                .map(|(_, notice)| notice.clone())
                .collect(),
        }
    }

    fn subscribe(&mut self, asker: &Outbox, attends: bool) -> Reply {
        if !self.subscribers.iter().any(|subscriber| subscriber.id == asker.id) {
            self.subscribers.push(asker.clone());
        }
        if attends {
            self.attending.insert(asker.id);
        }
        Reply { detail: Some(Box::new(Detail::Snapshot(self.snapshot()))), ..Reply::done() }
    }

    /// Whether a window is attending, which is what wakes the human (MIP-4, section 10).
    pub(crate) fn attended(&self) -> bool {
        self.subscribers.iter().any(|subscriber| self.attending.contains(&subscriber.id))
    }

    /// Whether a restore is still bringing back the panes a previous run saved.
    pub(crate) fn restoring(&self) -> bool {
        self.restoring
    }

    /// Each pane a tab's zoom hides, with the pane its tab is zoomed on.
    pub(crate) fn zoomed_away(&self) -> Vec<(String, String)> {
        let mut hidden = Vec::new();
        for tab in &self.tabs {
            let Some(zoomed) = &tab.zoomed else { continue };
            for pane in tab.root.panes().into_iter().filter(|pane| pane != zoomed) {
                hidden.push((pane.to_string(), zoomed.clone()));
            }
        }
        hidden
    }

    /// Tells the windows what now waits for the human, as the message service decided it at
    /// `order`. A group whose count is 0 has nothing waiting any more.
    pub(crate) fn tell_human(&mut self, order: u64, notices: Vec<proto::msg_answer::Notice>) {
        for notice in notices {
            let told = self.human.get(&notice.group);
            let unchanged = match told {
                Some((at, told)) => *at > order || *told == notice,
                // A group the human just joined is news with nothing waiting in it; one they
                // were never heard of in is not.
                None => notice.count == 0 && !notice.member,
            };
            if unchanged {
                continue;
            }
            // A group with nothing waiting stays, with its order, so that an older notice
            // arriving late is still known to be older.
            self.human.insert(notice.group.clone(), (order, notice.clone()));
            self.emit(Payload::HumanNotice(notice));
        }
    }

    /// Numbers an event and queues it for every subscriber. One that cannot take it has fallen
    /// too far behind to catch up from here, so it is dropped and resubscribes.
    fn emit(&mut self, payload: Payload) {
        if !matches!(
            payload,
            Payload::PaneEffect(_) | Payload::PasteHeld(_) | Payload::HumanNotice(_)
        ) {
            self.persister.changed();
        }
        self.seq += 1;
        let message = proto::ControlMessage {
            message: Some(proto::control_message::Message::Event(proto::Event {
                seq: self.seq,
                event: Some(payload),
            })),
        };
        let frame: Arc<[u8]> = message.encode_to_vec().into();
        self.subscribers.retain(|subscriber| subscriber.push(Arc::clone(&frame)));
    }

    // -----------------------------------------------------------------------------------------
    // Panes

    /// Checks a create and reserves its names. The pane is started outside the lock
    /// ([`Starting::start`]) and finished by [`Session::started`].
    fn create(&mut self, create: pane_request::Create) -> Handled {
        match self.prepare(create) {
            Ok(starting) => Handled::Start(Box::new(starting)),
            Err(reply) => Handled::Reply(reply),
        }
    }

    fn prepare(&mut self, create: pane_request::Create) -> Result<Starting, Reply> {
        valid_name("pane", &create.pane).map_err(Reply::refused)?;
        if self.pane_index(&create.pane).is_some() {
            return Err(Reply::already());
        }
        if self.reserved.contains(&create.pane) {
            return Err(Reply::refused(format!(
                "a pane {} is being started by another request",
                create.pane
            )));
        }
        if self.stopping {
            return Err(Reply::refused("the daemon is stopping"));
        }
        if create.command.as_deref() == Some("") {
            return Err(Reply::refused("the command is empty; leave it out to start a shell"));
        }
        let given = given_name(create.label.as_deref()).map(str::to_string);
        if let Some(given) = &given
            && let Some(taken) = self.name_taken(given, &create.pane)
        {
            return Err(taken);
        }
        if let Some(given) = &given
            && let Some(who) = self.joined_by(given, &create.pane)
        {
            return Err(name_joined(given, &who));
        }
        let maker = create.maker.filter(|maker| !maker.is_empty());
        if let Some(maker) = &maker {
            valid_name("maker", maker).map_err(Reply::refused)?;
            if *maker == create.pane {
                return Err(Reply::refused("a pane cannot be its own helper"));
            }
        }
        let target = self.target(create.placement, &create.pane, maker.as_deref())?;
        let neighbour = match &target {
            Target::Beside { pane, .. } => self.pane_index(pane).map(|index| &self.panes[index]),
            Target::NewTab { .. } | Target::Helpers { .. } => None,
        };
        let grid = match create.grid.map(grid) {
            Some(grid) => grid.map_err(Reply::refused)?,
            None => neighbour.map_or(Grid::FALLBACK, |pane| pane.io.grid()),
        };
        let cwd = match create.cwd.filter(|cwd| !cwd.is_empty()) {
            Some(cwd) => PathBuf::from(cwd),
            None => neighbour.and_then(Pane::live_cwd).unwrap_or_else(|| self.home.clone()),
        };

        let word = self.draw_word(given.as_deref());
        let name = given.clone().unwrap_or_else(|| word.clone());
        let (argv, environment) =
            self.launch(&create.pane, &name, create.command.as_deref(), &create.env);

        self.reserved.insert(create.pane.clone());
        if let Some(name) = target.new_tab() {
            self.reserved.insert(name.to_string());
        }
        self.naming.insert(word.clone(), create.pane.clone());
        if let Some(given) = given {
            self.naming.insert(given, create.pane.clone());
        }
        Ok(Starting {
            pane: create.pane,
            label: create.label,
            word,
            command: create.command,
            quiet: create.quiet,
            maker,
            target,
            grid,
            cwd,
            argv,
            environment,
        })
    }

    /// The program a pane starts, and its environment: the configured shell, running `command`
    /// first when there is one.
    fn launch(
        &self,
        pane: &str,
        name: &str,
        command: Option<&str>,
        requested: &HashMap<String, String>,
    ) -> Launched {
        let shell = self.settings.shell.clone().unwrap_or_default();
        self.launch_with(&shell, spawn::Which { pane, name }, command, requested)
    }

    /// As [`Session::launch`], under `shell` rather than the configured one.
    fn launch_with(
        &self,
        shell: &proto::Shell,
        which: spawn::Which<'_>,
        command: Option<&str>,
        requested: &HashMap<String, String>,
    ) -> Launched {
        let login = shell.mode() != proto::ShellMode::NonLogin;
        let environment = spawn::environment(
            &self.inherited,
            requested,
            which,
            command,
            &self.data,
            &self.reachable,
            spawn::Settings { shell, cursor: self.settings.cursor.as_ref() },
        );
        if shell.sudo.unwrap_or(false) {
            spawn::put_entry_in_home(&environment, &self.data);
        }
        spawn::start(
            &pty::shell(shell.command.as_deref(), &self.inherited),
            login,
            command.is_some(),
            environment,
            &self.data.shell_integration(),
        )
    }

    /// Finishes a create once its process has started, or failed to. The names it reserved are
    /// released either way, and a pane whose place went away while it started is ended again.
    pub(crate) fn started(
        &mut self,
        starting: Starting,
        started: std::io::Result<(OwnedFd, Child)>,
    ) -> Reply {
        self.reserved.remove(&starting.pane);
        if let Some(name) = starting.target.new_tab() {
            self.reserved.remove(name);
        }
        self.naming.retain(|_, pane| *pane != starting.pane);
        let program = &starting.argv[0];
        let (master, child) = match started {
            Ok(started) => started,
            Err(error) => {
                return Self::could_not_start(&starting.pane, program, &starting.cwd, &error);
            }
        };
        let gone = match &starting.target {
            _ if self.stopping => Some(Reply::refused("the daemon stopped while the pane started")),
            _ if self.replacing != Replacing::No => Some(Reply::refused(
                "the daemon began handing its panes to a new one while the pane started; ask again",
            )),
            Target::Beside { pane, .. } if self.tab_of(pane).is_none() => Some(Reply::not_there(
                format!("{pane} closed while the pane beside it was starting"),
            )),
            _ => None,
        };
        if let Some(reply) = gone {
            drop(master);
            pty::abandon(child.id().cast_signed());
            return reply;
        }

        let record = proto::Pane {
            pane: starting.pane.clone(),
            label: starting.label,
            word: Some(starting.word),
            cwd: starting.cwd.display().to_string(),
            command: starting.command,
            quiet: starting.quiet,
            maker: starting.maker,
            ..proto::Pane::default()
        };
        if let Err(reply) = self.open(record, starting.grid, master, child, program) {
            return reply;
        }
        self.place(&starting.pane, starting.target);
        Reply::done()
    }

    /// Watches a pane whose process has started, and announces it. It is in no tab yet.
    fn open(
        &mut self,
        record: proto::Pane,
        grid: Grid,
        master: OwnedFd,
        child: Child,
        program: &str,
    ) -> Result<(), Reply> {
        let name = record.pane.clone();
        let cwd = PathBuf::from(&record.cwd);
        let screen = match Screen::new(grid, &self.settled) {
            Ok(screen) => screen,
            Err(error) => {
                pty::abandon(child.id().cast_signed());
                let error = std::io::Error::other(error.to_string());
                return Err(Self::could_not_start(&name, program, &cwd, &error));
            }
        };
        self.next_serial += 1;
        let serial = self.next_serial;
        let watching = Watching {
            ended: &self.ended,
            reports: &self.reports,
            host: &self.host,
            detecting: &self.detecting,
            persister: &self.persister,
            held: false,
            detection: None,
            turns: Turns::default(),
        };
        let pane = Pane::start(
            record,
            serial,
            master,
            screen,
            grid,
            Some(Process::Child(child)),
            &watching,
        )
        .map_err(|error| Self::could_not_start(&name, program, &cwd, &error))?;
        log::info("daemon.pane.started", fields! { "pane" => name, "serial" => serial });
        self.emit(Payload::PaneOpened(proto::PaneOpened { pane: Some(pane.record.clone()) }));
        self.panes.push(pane);
        Ok(())
    }

    fn could_not_start(pane: &str, program: &str, cwd: &Path, error: &std::io::Error) -> Reply {
        log::warn(
            "daemon.pane.not_started",
            fields! {
                "pane" => pane,
                "program" => program,
                "cwd" => cwd.display(),
                "error" => error,
                "impact" => "no pane was made, and the create was refused",
                "check" => "that the directory exists and the shell is executable",
            },
        );
        Reply::refused(format!("could not start {program} in {}: {error}", cwd.display()))
    }

    /// Checks a placement against what is here. `moving` is the pane being placed, which a
    /// placement may not name as its own neighbour, and `maker` the pane it is a helper of.
    fn target(
        &self,
        placement: Option<proto::Placement>,
        moving: &str,
        maker: Option<&str>,
    ) -> Result<Target, Reply> {
        let Some(placement) = placement.and_then(|placement| placement.r#where) else {
            return Err(Reply::refused("the request does not say where the pane goes"));
        };
        match placement {
            proto::placement::Where::Beside(beside) => {
                if self.pane_index(&beside.pane).is_none() {
                    return Err(Reply::not_there(format!(
                        "no pane {} on this daemon",
                        beside.pane
                    )));
                }
                if beside.pane == moving {
                    return Err(Reply::refused("a pane cannot be placed beside itself"));
                }
                let side = side(beside.side).ok_or_else(|| {
                    Reply::refused("the side is not one of left, right, up or down")
                })?;
                let ratio = beside.ratio.map_or(Ok(0.5), valid_ratio).map_err(Reply::refused)?;
                Ok(Target::Beside { pane: beside.pane, side, ratio })
            }
            proto::placement::Where::NewTab(new_tab) => {
                self.free_tab(&new_tab.tab)?;
                Ok(Target::NewTab {
                    name: new_tab.tab,
                    label: new_tab.label.unwrap_or_default(),
                    hidden: new_tab.hidden,
                })
            }
            proto::placement::Where::Helpers(helpers) => {
                let Some(maker) = maker else {
                    return Err(Reply::refused(
                        "a pane placed among a maker's helpers needs a maker; say which pane it \
                         is a helper of",
                    ));
                };
                self.free_tab(&helpers.tab)?;
                Ok(Target::Helpers {
                    maker: maker.to_string(),
                    name: helpers.tab,
                    label: helpers.label.unwrap_or_default(),
                })
            }
        }
    }

    /// Refuses a name for a new tab that is not one, or is already here or being made.
    fn free_tab(&self, tab: &str) -> Result<(), Reply> {
        valid_name("tab", tab).map_err(Reply::refused)?;
        if self.tab_index(tab).is_some() || self.reserved.contains(tab) {
            return Err(Reply::refused(format!(
                "a tab {tab} is already on this daemon; place the pane beside one of its panes"
            )));
        }
        Ok(())
    }

    /// The first tab a new helper of `maker` joins: a hidden one holding a helper of it, or one
    /// on screen holding nothing but its helpers, which Show Helpers brought forward.
    fn helpers_tab(&self, maker: &str) -> Option<usize> {
        let helps = |pane: &str| {
            self.pane_index(pane)
                .is_some_and(|index| self.panes[index].record.maker.as_deref() == Some(maker))
        };
        self.tabs.iter().position(|tab| {
            let panes = tab.root.panes();
            if tab.hidden { panes.into_iter().any(helps) } else { panes.into_iter().all(helps) }
        })
    }

    /// Puts a pane that is in no tab where `target` says, and announces the tab.
    fn place(&mut self, pane: &str, target: Target) {
        match target {
            Target::Beside { pane: beside, side, ratio } => {
                let index = self.tab_of(&beside).expect("a checked neighbour is in a tab");
                self.tabs[index].root.insert(&beside, pane, side, ratio);
                let record = self.tabs[index].record();
                self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
            }
            Target::NewTab { name, label, hidden } => {
                let tab =
                    Tab { name, label, root: Node::Pane(pane.to_string()), zoomed: None, hidden };
                let record = tab.record();
                self.tabs.push(tab);
                self.emit(Payload::TabOpened(proto::TabOpened { tab: Some(record) }));
            }
            Target::Helpers { maker, name, label } => match self.helpers_tab(&maker) {
                Some(index) => {
                    self.tabs[index].root.append_column(pane);
                    let record = self.tabs[index].record();
                    self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
                }
                None => self.place(pane, Target::NewTab { name, label, hidden: true }),
            },
        }
    }

    /// Takes a pane out of its tab's tree, closing the tab if that emptied it. Returns the
    /// tab's event, for the caller to emit when its own changes are done.
    fn lift(&mut self, pane: &str) -> Payload {
        let index = self.tab_of(pane).expect("every pane is in a tab");
        let tab = &mut self.tabs[index];
        let root = std::mem::replace(&mut tab.root, Node::Pane(String::new()));
        if tab.zoomed.as_deref() == Some(pane) {
            tab.zoomed = None;
        }
        if let Some(rest) = root.without(pane).0 {
            tab.root = rest;
            return Payload::TabChanged(proto::TabChanged { tab: Some(tab.record()) });
        }
        let closed = self.tabs.remove(index);
        Payload::TabClosed(proto::TabClosed { tab: closed.name })
    }

    /// A refusal for a report whose sender's nearest agent is not the pane's own: one running
    /// outside the process group the pane's agent was found in. And for any report that came
    /// through a harness's shared server, whatever pane it names, since the pane it names is the
    /// one whose session started the server ([`crate::attribution`]). None - take the report - for
    /// anything else, including a pane with no agent identified or no group known for it.
    pub(crate) fn foreign_report(
        &self,
        service: &Service,
        sender: &crate::attribution::Sender,
    ) -> Option<Reply> {
        let Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::Report(report)),
        }) = service
        else {
            return None;
        };
        let sender = match sender {
            crate::attribution::Sender::Server(server) => {
                crate::attribution::refused_through_server(&report.pane, server);
                return Some(Reply::refused(server.refusal()));
            }
            crate::attribution::Sender::Agent(nearest) => nearest,
        };
        let pane = &self.panes[self.pane_index(&report.pane)?];
        let own = pane.record.agent.as_deref()?;
        if pane.io.agent_group()? == sender.group {
            return None;
        }
        crate::attribution::refused(&report.pane, own, sender);
        Some(Reply::refused(format!(
            "pane {} runs {own}, and this report came from another {} (pid {}), outside the \
             process group the pane's own agent runs in; it inherited the pane's environment, \
             and its reports are not the pane's",
            report.pane, sender.agent, sender.pid
        )))
    }

    /// Facts the agent in a pane states about itself.
    fn report(&mut self, mut report: pane_request::Report) -> Reply {
        let Some(index) = self.pane_index(&report.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", report.pane));
        };
        let own_state = match report.state.take().map(proto::AgentState::try_from) {
            None => None,
            Some(_) if report.agent.is_empty() => {
                return Reply::refused("a state needs the name of the agent reporting it");
            }
            Some(Ok(
                state @ (proto::AgentState::Working
                | proto::AgentState::Blocked
                | proto::AgentState::Idle),
            )) => Some(detect::state_of(state)),
            Some(_) => return Reply::refused("an agent reports itself working, blocked or idle"),
        };
        let agent = std::mem::take(&mut report.agent);
        let mut session_name = report.session_name.take();
        if session_name.is_some() && agent.is_empty() {
            return Reply::refused("a session's name needs the name of the agent reporting it");
        }
        // Left out rather than refused: a statusline sends the context, model and cost beside it,
        // and a refusal would drop them all, every time it runs.
        if let Some(why) = session_name.as_deref().and_then(|name| facts::session_name(name).err())
        {
            log::debug(
                "daemon.report.session_name_ignored",
                fields! { "pane" => report.pane, "why" => why },
            );
            session_name = None;
        }
        let session_id = report.session_id.take();
        if session_id.is_some() && agent.is_empty() {
            return Reply::refused("a session's id needs the name of the agent reporting it");
        }
        if let Some(why) = session_id.as_deref().and_then(|id| facts::session_id(id).err()) {
            return Reply::refused(why);
        }
        let cleared = report.clear;
        let context_used = report.context_used;
        let says_waiting = report.waiting.as_deref().is_some_and(|waiting| !waiting.is_empty());
        let speaks_of_waiting = report.waiting.is_some();
        let record = &mut self.panes[index].record;
        let facts = match facts::apply(record.facts.as_ref(), report) {
            Ok(facts) => facts,
            Err(why) => return Reply::refused(why),
        };
        let facts_changed = record.facts != facts;
        let heard = adapter_heard(record, &agent);
        if facts_changed {
            let declared = is_waiting(facts.as_ref()) && !is_waiting(record.facts.as_ref());
            let withdrawn = !is_waiting(facts.as_ref()) && is_waiting(record.facts.as_ref());
            record.facts = facts;
            if withdrawn {
                log::debug(
                    "daemon.report.waiting_cleared",
                    fields! { "pane" => record.pane, "by" => "report" },
                );
            }
            if declared {
                // An agent waiting on its own work has not finished.
                record.finished_unseen = false;
                log::debug(
                    "daemon.report.waiting",
                    fields! { "pane" => record.pane, "agent" => agent.as_str() },
                );
            }
        }
        if facts_changed || heard {
            let record = record.clone();
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
        let renamed = self.session_named(index, cleared, &agent, session_name.as_deref());
        let identified = self.session_identified(index, &agent, session_id);
        let compacting = self.past_compact_at(index, cleared, context_used);
        self.compaction_changed(index);
        let pane = &mut self.panes[index];
        if says_waiting {
            pane.turns.wait_declared = true;
        }
        // What the agent says it waits on is its own word, whatever its screen showed.
        if speaks_of_waiting {
            pane.turns.wait_on_screen = false;
        }
        // Detection decides what the state comes to, and publishes it as any other change.
        if let Some(state) = own_state {
            if pane.record.agent.as_deref() == Some(agent.as_str()) {
                pane.turns.reports_turns = true;
                if state == muster_detect::State::Idle
                    && settle_wait(&mut pane.record, &mut pane.turns, "reported turn end")
                {
                    // The turn that ended the wait has finished. Detection marks a finish when it
                    // sees the agent stop, and a pane it already reads idle shows it no stop.
                    if pane.record.agent_state() == proto::AgentState::Idle {
                        pane.record.finished_unseen = true;
                    }
                    let record = pane.record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
            }
            self.panes[index].io.report_state(agent, state);
            return Reply::done();
        }
        // Said again, a wait outlasts one more turn, which is a change even in the same words.
        if facts_changed || heard || says_waiting || renamed || identified || compacting {
            Reply::done()
        } else {
            Reply::already()
        }
    }

    /// What a report of the context says against `compact_at` ([`crate::compaction`]); true when
    /// it crossed it and a compaction is now to be typed, which the doorbell is told of.
    fn past_compact_at(&mut self, index: usize, cleared: bool, context_used: Option<f32>) -> bool {
        let threshold = self.settings.compact_at;
        let manifests = self.detecting.manifests();
        let pane = &mut self.panes[index];
        if cleared {
            pane.compaction.cleared();
        }
        let Some(used) = context_used else { return false };
        if !pane.compaction.reported(used, threshold) {
            return false;
        }
        let agent = pane.record.agent.clone().unwrap_or_default();
        let line = manifests.and_then(|manifests| {
            manifests.session_compact(&muster_detect::Agent::new(&agent), None)?.ok()
        });
        let Some(line) = line else {
            log::debug(
                "daemon.compact.cannot",
                fields! {
                    "pane" => pane.record.pane,
                    "agent" => agent,
                    "why" => "its manifest says no way to compact it",
                },
            );
            return false;
        };
        log::info(
            "daemon.compact.past_threshold",
            fields! { "pane" => pane.record.pane, "agent" => agent, "context_used" => used },
        );
        pane.compaction.crossed(line);
        pane.compaction.wanted().is_some()
    }

    /// Forgets the session id a pane's agent reported, if it is still `id`: a wake the command
    /// could not deliver by it is typed instead, until the agent reports one again.
    pub(crate) fn forget_session_id(&mut self, pane: &str, id: &str) {
        if let Some(index) = self.pane_index(pane)
            && self.panes[index].session_id.as_deref() == Some(id)
        {
            self.panes[index].session_id = None;
            self.session_described(index);
        }
    }

    /// Publishes the pane's record when its session or how it resumes has moved on it.
    fn session_described(&mut self, index: usize) {
        if self.panes[index].describe_session() {
            let record = self.panes[index].record.clone();
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
    }

    /// Keeps the session id a report says, while its agent is the pane's or before detection has
    /// found one, as a session's name counts; true when it changed. Not forgotten by `--clear`,
    /// which a harness sends as a session starts, perhaps after the new session's id.
    fn session_identified(&mut self, index: usize, agent: &str, said: Option<String>) -> bool {
        let pane = &mut self.panes[index];
        let Some(said) = said else { return false };
        if pane.record.agent.as_deref().is_some_and(|found| found != agent) {
            return false;
        }
        let id = Some(said).filter(|id| !id.is_empty());
        let changed = pane.session_id != id;
        // The same session again is worth another try at its resume when the last one could not
        // be built: the job was between processes, or its arguments would not read.
        if !changed && (id.is_none() || pane.resume.is_some()) {
            return false;
        }
        if changed {
            log::debug(
                "session.id.reported",
                fields! { "pane" => pane.record.pane, "agent" => agent, "known" => id.is_some() },
            );
            pane.session_id.clone_from(&id);
        }
        let resume = id.as_deref().and_then(|session| self.resume_of(index, agent, session));
        if let Some(session) = id.as_deref().filter(|_| resume.is_none()) {
            self.resume_missed(index, agent, session);
        }
        if resume.is_some() || changed {
            self.resume_changed(index, resume);
        }
        self.session_described(index);
        changed
    }

    /// Says once per session that its resume could not be built, which otherwise leaves a pane
    /// that will come back after a restart as a shell with nothing to say why.
    fn resume_missed(&mut self, index: usize, agent: &str, session: &str) {
        // A harness whose manifest says no way to resume has nothing to have missed.
        let resumable = self.detecting.manifests().is_some_and(|manifests| {
            manifests
                .session_resume(&muster_detect::Agent::new(agent), session, &[], None)
                .is_some()
        });
        let pane = &mut self.panes[index];
        if !resumable || pane.resume_missed.as_deref() == Some(session) {
            return;
        }
        pane.resume_missed = Some(session.to_string());
        log::warn(
            "session.resume.unbuilt",
            fields! {
                "pane" => pane.record.pane,
                "agent" => agent,
                "impact" => "after a daemon restart this pane comes back as a shell, not running \
                             its agent's session; tried again at the agent's next report",
                "check" => "whether the agent's manifest says how to resume, and whether its \
                            process was in the pane's foreground with readable arguments when \
                            it reported",
            },
        );
    }

    /// The command that starts the session `session` of the pane's agent again after a restart:
    /// its manifest's resume, or the one a wrapper named in the agent's `MUSTER_RESUME`, with
    /// the arguments the agent process is running with now, read off the pane's foreground job.
    /// Read now because a session is reported from inside the agent, so it is running; after a
    /// restart there is nothing left to read. None when the manifest does not say how, or the
    /// agent's process will not say what it was started with.
    fn resume_of(&self, index: usize, agent: &str, session: &str) -> Option<persist::Resume> {
        use muster_detect::Processes as _;
        let manifests = self.detecting.manifests()?;
        let io = &self.panes[index].io;
        let group = u32::try_from(io.foreground_group()?).ok()?;
        let shell = io.shell().and_then(|shell| u32::try_from(shell).ok()).unwrap_or(group);
        let job = muster_detect::System.job(shell, group)?;
        let (found, arguments) = muster_detect::agent_arguments(&job, &manifests)?;
        if found != muster_detect::Agent::new(agent) {
            return None;
        }
        let named = muster_detect::agent_process(&job, &manifests)
            .and_then(|(_, process)| muster_detect::System.resume_hint(process.pid));
        let mut resume = manifests.session_resume(&found, session, &arguments, named.as_deref())?;
        if let Some(why) = &resume.ignored {
            log::warn(
                "session.resume.named_unusable",
                fields! {
                    "pane" => self.panes[index].record.pane,
                    "agent" => agent,
                    "why" => why,
                    "impact" => "after a daemon restart this pane resumes its session through \
                                 the agent's manifest rather than the command its wrapper \
                                 named, so whatever the wrapper sets up is not set up again",
                    "check" => "the MUSTER_RESUME the wrapper exports: one program and its \
                                arguments, separated by spaces, with {session} as a word of \
                                its own",
                },
            );
        }
        // The agent's own executable where it was started from a path, so an agent an alias runs
        // from outside `PATH` is the one resumed, rather than whichever the name finds there.
        if let Some(program) = muster_detect::agent_program(&job, &manifests)
            && let Some(first) = resume.command.first_mut()
            && Path::new(&program).file_name() == Some(std::ffi::OsStr::new(first.as_str()))
        {
            *first = program;
        }
        Some(persist::Resume {
            agent: agent.to_string(),
            session: session.to_string(),
            command: resume.command,
            uncarried: resume.uncarried,
        })
    }

    /// Keeps what starts the pane's agent session again, saving it when it changed.
    fn resume_changed(&mut self, index: usize, resume: Option<persist::Resume>) {
        if self.panes[index].resume != resume {
            self.panes[index].resume = resume;
            self.persister.changed();
        }
    }

    /// What a report says of the session's name, kept in step with the pane's
    /// ([`crate::session_name`]); true when the pane took the session's name, or a name is now to
    /// be typed into the session that was not before - both are what the doorbell is told of. A name counts while
    /// its agent is the pane's, or before detection has found one: a statusline can report before
    /// the first probe lands.
    fn session_named(
        &mut self,
        index: usize,
        cleared: bool,
        agent: &str,
        said: Option<&str>,
    ) -> bool {
        let pane = &mut self.panes[index];
        let label = pane.record.label.clone();
        let wanted = pane.session_name.wanted().map(str::to_string);
        if cleared {
            pane.session_name.cleared(&pane.record.pane, label.as_deref());
        }
        let ours = pane.record.agent.as_deref().is_none_or(|found| found == agent);
        let taken = said
            .filter(|_| ours)
            .and_then(|said| pane.session_name.heard(&pane.record.pane, said, label.as_deref()));
        let Some(taken) = taken else {
            return pane.session_name.wanted() != wanted.as_deref();
        };
        let changed = pane.session_name.wanted() != wanted.as_deref();
        let name = pane.record.pane.clone();
        if let Some(holder) = given_name(Some(&taken)).and_then(|taken| self.called(taken, &name)) {
            log::info(
                "session.name.held",
                fields! {
                    "pane" => name,
                    "agent" => agent,
                    "holder" => holder,
                    "impact" => "the pane keeps its name, since another pane here is called what \
                                 its agent's session is",
                },
            );
            return changed;
        }
        if let Some(holder) =
            given_name(Some(&taken)).and_then(|taken| self.joined_by(taken, &name))
        {
            log::info(
                "session.name.held",
                fields! {
                    "pane" => name,
                    "agent" => agent,
                    "holder" => holder,
                    "impact" => "the pane keeps its name, since an agent here joined messaging by \
                                 what its agent's session is called, and a post to it would \
                                 mean both",
                },
            );
            return changed;
        }
        let pane = &mut self.panes[index];
        log::info("session.name.taken", fields! { "pane" => pane.record.pane, "agent" => agent });
        pane.record.label = Some(taken);
        let record = pane.record.clone();
        self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        true
    }

    /// Clears the finish nobody had seen on each named pane there is. A pane named that is not
    /// there, as one a window showed can close while it asks, is said in the answer and does not
    /// stop the others being seen; only a request naming no pane there is refused.
    fn seen(&mut self, panes: &[String]) -> Reply {
        let missing: Vec<&str> = panes
            .iter()
            .filter(|pane| self.pane_index(pane).is_none())
            .map(String::as_str)
            .collect();
        if !panes.is_empty() && missing.len() == panes.len() {
            return Reply::not_there(format!("no pane {} on this daemon", missing.join(", ")));
        }
        let mut cleared = Vec::new();
        for pane in &mut self.panes {
            if pane.record.finished_unseen && panes.contains(&pane.record.pane) {
                pane.record.finished_unseen = false;
                cleared.push(pane.record.clone());
            }
        }
        let mut reply = if cleared.is_empty() { Reply::already() } else { Reply::done() };
        if !missing.is_empty() {
            reply.reason = format!("no pane {} on this daemon", missing.join(", "));
        }
        for record in cleared {
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
        reply
    }

    fn close_pane(&mut self, pane: &str) -> Reply {
        if self.pane_index(pane).is_none() {
            return Reply::not_there(format!("no pane {pane} on this daemon"));
        }
        self.remove(pane, proto::CloseReason::Requested, None);
        Reply::done()
    }

    /// Something a pane's program asked for, from the publisher. Nothing to do if the pane has
    /// closed since.
    pub(crate) fn reported(&mut self, report: Report) {
        let Some(index) = self.panes.iter().position(|pane| pane.serial == report.serial) else {
            return;
        };
        let name = self.panes[index].record.pane.clone();
        let pane = &mut self.panes[index];
        let record = &mut pane.record;
        // What sets one field of the record, and is news only when it changes it.
        let set = match report.what {
            Reported::Title(title) => Ok(replaced(&mut record.title, title)),
            Reported::Cwd(directory) => {
                Ok(replaced(&mut record.cwd, directory.display().to_string()))
            }
            Reported::ShiftCapture(capture) => Ok(replaced(&mut record.shift_capture, capture)),
            Reported::UnreadInput(unread) => Ok(replaced(&mut record.unread_input, unread)),
            what => Err(what),
        };
        let what = match set {
            Ok(changed) => {
                if changed {
                    let record = record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
                return;
            }
            Err(what) => what,
        };
        match what {
            // Set above; and a settle is answered by the publisher, never applied here.
            Reported::Title(_)
            | Reported::Cwd(_)
            | Reported::ShiftCapture(_)
            | Reported::UnreadInput(_)
            | Reported::Settled(_) => {}
            Reported::Shown(effect) => {
                self.emit(Payload::PaneEffect(proto::PaneEffect {
                    pane: name,
                    effect: Some(effect),
                }));
            }
            Reported::PasteHeld(text) => {
                self.emit(Payload::PasteHeld(proto::PasteHeld { pane: name, text }));
            }
            Reported::Agent { agent, state, reported, unreadable, waiting: on_screen } => {
                let before = record.clone();
                // What an agent said about itself leaves with it. Not when a pane with no agent
                // is first recognised: its statusline can report before the first probe lands,
                // and that report is the new agent's.
                let replaced = record.agent.is_some() && record.agent != agent;
                if replaced {
                    record.facts = None;
                    record.set_adapter(proto::Adapter::Unsaid);
                    pane.turns.reports_turns = false;
                    pane.session_id = None;
                    if pane.resume.take().is_some() {
                        self.persister.changed();
                    }
                    (record.session_id, record.resume) = (None, None);
                    let left = record.agent.as_deref().unwrap_or_default();
                    let restarting = pane.io.discards().is_restarting();
                    forget_compaction(&mut pane.compaction, &name, left, restarting);
                    record.set_pending_compaction(proto::PendingCompaction::None);
                }
                if replaced || (record.agent.is_none() && agent.is_some()) {
                    pane.session_name.agent_changed(&name, replaced, record.label.as_deref());
                }
                let turn = Turn::between(record.agent_state(), state);
                // An agent that reports its own state ends its own turns: a sub-agent's tool
                // call can read as a turn here after the agent's turn has ended.
                if turn == Turn::Ended && !pane.turns.reports_turns {
                    settle_wait(record, &mut pane.turns, "detected turn end");
                }
                // Before a finish is weighed: a turn that ends waiting on its own work has not
                // finished.
                wait_on_screen(record, &mut pane.turns, on_screen);
                // Every adapter reports by the end of a turn, so one that has not by now is not
                // installed, or not running.
                if turn == Turn::Ended
                    && record.agent == agent
                    && record.adapter() == proto::Adapter::Unsaid
                {
                    record.set_adapter(proto::Adapter::Silent);
                }
                let waiting = is_waiting(record.facts.as_ref());
                if turn == Turn::Ended && record.agent == agent {
                    turn_ended(&name, agent.as_deref(), waiting);
                }
                if !waiting {
                    record.finished_unseen = finished_unseen(
                        (record.agent.as_deref(), record.agent_state()),
                        (agent.as_deref(), state),
                        record.finished_unseen,
                    );
                }
                record.agent_compacts =
                    agent_compacts(self.detecting.manifests().as_deref(), agent.as_deref());
                record.agent = agent;
                record.set_agent_state(state);
                record.state_reported = reported;
                record.screen_unreadable = unreadable;
                if record.screen_unreadable && !before.screen_unreadable {
                    unreadable_warning(&name, record.agent.as_deref());
                }
                if *record != before {
                    let record = record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
            }
        }
    }

    /// A pane's process ended. Nothing to do if the pane was already closed.
    fn ended(&mut self, serial: u64, status: Option<i32>) {
        // The pane is the new daemon's now, and so is saying it ended.
        if self.replacing == Replacing::HandedOff {
            return;
        }
        let Some(index) = self.panes.iter().position(|pane| pane.serial == serial) else { return };
        let name = self.panes[index].record.pane.clone();
        log::info(
            "daemon.pane.exited",
            fields! { "pane" => name, "status" => format!("{status:?}") },
        );
        self.remove(&name, proto::CloseReason::Exited, status);
    }

    fn remove(&mut self, pane: &str, reason: proto::CloseReason, exit_status: Option<i32>) {
        let tab_event = self.lift(pane);
        self.emit(tab_event);
        self.forget(pane, reason, exit_status);
    }

    /// Drops a pane that is in no tree any more, and hangs it up.
    fn forget(&mut self, pane: &str, reason: proto::CloseReason, exit_status: Option<i32>) {
        let Some(index) = self.pane_index(pane) else { return };
        let removed = self.panes.remove(index);
        let maker = &removed.record;
        self.orphan(&maker.pane, maker.name().unwrap_or(&maker.pane));
        removed.io.mark_closed();
        self.emit(Payload::PaneClosed(proto::PaneClosed {
            pane: pane.to_string(),
            reason: reason.into(),
            exit_status,
        }));
        let reason = match reason {
            proto::CloseReason::Exited => proto::DetachReason::Exited,
            _ => proto::DetachReason::Closed,
        };
        self.deferred.push(Deferred::HangUp(Box::new(removed), reason));
    }

    /// When `maker`, called `called` as it closed, was the maker of helpers still open here that
    /// are not yet marked, marks each and lets it ask for the person again, and keeps the maker
    /// for the person to be told of (MIP-7, section 6): they go on running, and nobody is left
    /// to hear from them. Says whether there were any.
    ///
    /// A daemon that is stopping orphans nothing: its helpers close with their maker, and both
    /// come back from the state file it wrote as it began to stop.
    fn orphan(&mut self, maker: &str, called: &str) -> bool {
        if self.stopping {
            return false;
        }
        let mut helpers = Vec::new();
        for index in 0..self.panes.len() {
            let record = &mut self.panes[index].record;
            if record.maker.as_deref() != Some(maker) || record.maker_closed {
                continue;
            }
            let name = record.name().unwrap_or(&record.pane).to_string();
            helpers.push((record.pane.clone(), name));
            record.maker_closed = true;
            record.quiet = false;
            let record = record.clone();
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
        if helpers.is_empty() {
            return false;
        }
        let names: Vec<&str> = helpers.iter().map(|(_, name)| name.as_str()).collect();
        log::info(
            "daemon.maker.closed",
            fields! { "maker" => maker, "helpers" => names.join(",") },
        );
        self.orphans.push(Orphaned {
            maker: maker.to_string(),
            called: called.to_string(),
            helpers,
        });
        true
    }

    /// A maker on another machine has closed, as a window saw ([`Self::orphan`]).
    fn maker_closed(&mut self, closed: &pane_request::MakerClosed) -> Reply {
        if self.pane_index(&closed.maker).is_some() {
            return Reply::refused(format!(
                "pane {} is open on this daemon, so it has not closed",
                closed.maker
            ));
        }
        let called = closed.called.as_deref().unwrap_or(&closed.maker);
        if self.orphan(&closed.maker, called) { Reply::done() } else { Reply::already() }
    }

    /// The makers that closed with helpers open here since this was last asked.
    pub(crate) fn take_orphans(&mut self) -> Vec<Orphaned> {
        std::mem::take(&mut self.orphans)
    }

    fn resize(&mut self, resize: &pane_request::Resize) -> Reply {
        let Some(index) = self.tab_of(&resize.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", resize.pane));
        };
        let Some(direction) = side(resize.direction) else {
            return Reply::refused("the direction is not one of left, right, up or down");
        };
        match self.tabs[index].root.resize(&resize.pane, direction, resize.fraction) {
            Resized::Moved => {
                let record = self.tabs[index].record();
                self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
                Reply::done()
            }
            Resized::AtLimit => Reply::already(),
            Resized::NoDivider => Reply::refused(format!(
                "no split on that axis holds {}, so there is no divider to move",
                resize.pane
            )),
            Resized::Absent => unreachable!("the pane's tab was just found"),
        }
    }

    fn zoom(&mut self, zoom: &pane_request::Zoom) -> Reply {
        let Some(index) = self.tab_of(&zoom.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", zoom.pane));
        };
        let tab = &mut self.tabs[index];
        let is_zoomed = tab.zoomed.as_deref() == Some(zoom.pane.as_str());
        if is_zoomed == zoom.zoomed {
            return Reply::already();
        }
        tab.zoomed = zoom.zoomed.then(|| zoom.pane.clone());
        let record = tab.record();
        self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
        Reply::done()
    }

    fn swap(&mut self, swap: &pane_request::Swap) -> Reply {
        let (Some(index), Some(other)) = (self.tab_of(&swap.pane), self.tab_of(&swap.with)) else {
            return Reply::not_there(format!(
                "{} and {} are not both on this daemon",
                swap.pane, swap.with
            ));
        };
        if swap.pane == swap.with {
            return Reply::already();
        }
        if index != other {
            return Reply::refused("the two panes are in different tabs; move one instead");
        }
        self.tabs[index].root.swap(&swap.pane, &swap.with);
        let record = self.tabs[index].record();
        self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
        Reply::done()
    }

    fn move_pane(&mut self, moved: pane_request::Move) -> Reply {
        let Some(source) = self.tab_of(&moved.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", moved.pane));
        };
        if matches!(
            moved.placement.as_ref().and_then(|placement| placement.r#where.as_ref()),
            Some(proto::placement::Where::Helpers(_))
        ) {
            return Reply::refused(
                "only a pane being made is placed among a maker's helpers; move this one beside \
                 one of them instead",
            );
        }
        let target = match self.target(moved.placement, &moved.pane, None) {
            Ok(target) => target,
            Err(reply) => return reply,
        };
        let source_name = self.tabs[source].name.clone();
        let staying = matches!(&target, Target::Beside { pane, .. }
            if self.tab_of(pane) == Some(source));
        let source_event = self.lift(&moved.pane);
        if !staying {
            self.emit(source_event);
        }
        self.place(&moved.pane, target);
        log::info("daemon.pane.moved", fields! { "pane" => moved.pane, "from" => source_name });
        Reply::done()
    }

    fn rename_pane(&mut self, rename: pane_request::Rename) -> Reply {
        let Some(index) = self.pane_index(&rename.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", rename.pane));
        };
        if self.panes[index].record.label == rename.label {
            return Reply::already();
        }
        let given = given_name(rename.label.as_deref());
        if let Some(given) = given
            && let Some(taken) = self.name_taken(given, &rename.pane)
        {
            return taken;
        }
        if let Some(given) = given
            && let Some(who) = self.joined_by(given, &rename.pane)
        {
            return name_joined(given, &who);
        }
        // Back to its word, which another pane may have been named since.
        let word = self.panes[index].record.word.clone();
        let redrawn = match (given, word) {
            (None, Some(word)) if self.called(&word, &rename.pane).is_some() => {
                Some(self.draw_word(None))
            }
            _ => None,
        };
        let record = &mut self.panes[index].record;
        record.label = rename.label;
        if redrawn.is_some() {
            record.word = redrawn;
        }
        let record = record.clone();
        let pane = &mut self.panes[index];
        pane.session_name.named(&record.pane, record.label.as_deref());
        self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        Reply::done()
    }

    fn quiet(&mut self, quiet: &pane_request::Quiet) -> Reply {
        let Some(index) = self.pane_index(&quiet.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", quiet.pane));
        };
        let record = &mut self.panes[index].record;
        if record.quiet == quiet.quiet {
            return Reply::already();
        }
        record.quiet = quiet.quiet;
        let record = record.clone();
        self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        Reply::done()
    }

    // -----------------------------------------------------------------------------------------
    // Tabs

    fn close_tab(&mut self, name: &str) -> Reply {
        let Some(index) = self.tab_index(name) else {
            return Reply::not_there(format!("no tab {name} on this daemon"));
        };
        let tab = self.tabs.remove(index);
        self.emit(Payload::TabClosed(proto::TabClosed { tab: tab.name.clone() }));
        for pane in tab.root.panes() {
            self.forget(pane, proto::CloseReason::Requested, None);
        }
        Reply::done()
    }

    fn set_tab_hidden(&mut self, set: tab_request::SetHidden) -> Reply {
        let Some(index) = self.tab_index(&set.tab) else {
            return Reply::not_there(format!("no tab {} on this daemon", set.tab));
        };
        let tab = &mut self.tabs[index];
        let label = set
            .label
            .filter(|label| label.generation >= tab.label.generation && *label != tab.label);
        if tab.hidden == set.hidden && label.is_none() {
            return Reply::already();
        }
        tab.hidden = set.hidden;
        if let Some(label) = label {
            tab.label = label;
        }
        let record = tab.record();
        self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
        Reply::done()
    }

    fn rename_tab(&mut self, rename: tab_request::Rename) -> Reply {
        let Some(index) = self.tab_index(&rename.tab) else {
            return Reply::not_there(format!("no tab {} on this daemon", rename.tab));
        };
        let Some(label) = rename.label else {
            return Reply::refused("the rename carries no label");
        };
        let tab = &mut self.tabs[index];
        if label.generation < tab.label.generation {
            return Reply::refused(format!(
                "this tab's name is at generation {}, newer than the {} this rename carries",
                tab.label.generation, label.generation
            ));
        }
        if label == tab.label {
            return Reply::already();
        }
        tab.label = label;
        let record = tab.record();
        self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
        Reply::done()
    }

    fn set_split_ratio(&mut self, set: &tab_request::SetSplitRatio) -> Reply {
        let Some(index) = self.tab_index(&set.tab) else {
            return Reply::not_there(format!("no tab {} on this daemon", set.tab));
        };
        let path: Option<Vec<tree::Branch>> = set.path.iter().map(|&step| branch(step)).collect();
        let Some(path) = path else {
            return Reply::refused("a step in the path is neither first nor second");
        };
        match self.tabs[index].root.set_ratio(&path, set.ratio) {
            Ok(true) => {
                let record = self.tabs[index].record();
                self.emit(Payload::TabChanged(proto::TabChanged { tab: Some(record) }));
                Reply::done()
            }
            Ok(false) => Reply::already(),
            Err(why) => Reply::refused(why),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Settings

    fn set_shell(&mut self, set: proto::SetShell) -> Reply {
        let Some(shell) = set.shell else {
            return Reply::refused("the request carries no shell");
        };
        if shell.command.as_deref() == Some("") {
            return Reply::refused("the shell is empty; leave it out for the account's own");
        }
        if self.settings.shell.as_ref() == Some(&shell) {
            return Reply::already();
        }
        self.settings.shell = Some(shell);
        self.settings_changed()
    }

    fn set_scrollback(&mut self, set: proto::SetScrollback) -> Reply {
        if self.settings.scrollback_bytes == set.bytes {
            return Reply::already();
        }
        self.settings.scrollback_bytes = set.bytes;
        self.resettle();
        self.settings_changed()
    }

    /// Numbers what the settings now mean for a pane's terminal, and has every pane apply it
    /// once the lock is let go.
    fn resettle(&mut self) {
        self.settled = Arc::new(Settled {
            generation: self.settled.generation + 1,
            appearance: Appearance::of(&self.settings),
            scrollback: scrollback(&self.settings),
            scroll_multiplier: self.settings.scroll_multiplier.unwrap_or(1.0),
        });
        for pane in &self.panes {
            self.deferred.push(Deferred::Settle(Arc::clone(&pane.io), Arc::clone(&self.settled)));
        }
    }

    fn set_palette(&mut self, set: proto::SetPalette) -> Reply {
        let Some(palette) = set.palette else {
            return Reply::refused("the request carries no palette");
        };
        if palette.entries.len() > 256 {
            return Reply::refused(format!(
                "a palette has at most 256 entries, and this one has {}",
                palette.entries.len()
            ));
        }
        if self.settings.palette.as_ref() == Some(&palette) {
            return Reply::already();
        }
        self.settings.palette = Some(palette);
        self.resettle();
        self.settings_changed()
    }

    fn set_clipboard_write(&mut self, set: proto::SetClipboardWrite) -> Reply {
        if self.settings.clipboard_write.unwrap_or(true) == set.allowed {
            return Reply::already();
        }
        self.settings.clipboard_write = Some(set.allowed);
        self.resettle();
        self.settings_changed()
    }

    fn set_resume_agents(&mut self, resume: bool) -> Reply {
        if self.resumes_agents() == resume {
            return Reply::already();
        }
        self.settings.resume_agents = Some(resume);
        self.settings_changed()
    }

    /// Whether a restart brings a pane back running its agent's session (`SetResumeAgents`).
    pub(crate) fn resumes_agents(&self) -> bool {
        self.settings.resume_agents.unwrap_or(true)
    }

    fn set_name_sessions(&mut self, name: bool) -> Reply {
        if self.names_sessions() == name {
            return Reply::already();
        }
        self.settings.name_sessions = Some(name);
        self.settings_changed()
    }

    /// How full an agent's context gets before it is compacted unasked. None turns it off, and
    /// takes back every compaction it asked for that is not typed yet.
    fn set_compact_at(&mut self, percent: Option<f32>) -> Reply {
        if let Some(percent) = percent
            && !(percent > 0.0 && percent <= 100.0)
        {
            return Reply::refused(format!(
                "compact_at is {percent}, and a context is above 0 and at most 100 percent full"
            ));
        }
        if self.settings.compact_at.map(f32::to_bits) == percent.map(f32::to_bits) {
            return Reply::already();
        }
        self.settings.compact_at = percent;
        if percent.is_none() {
            for index in 0..self.panes.len() {
                self.panes[index].compaction.threshold_off();
                self.compaction_changed(index);
            }
        }
        self.settings_changed()
    }

    /// An empty name is none: the human is shown by the address alone.
    fn set_human_name(&mut self, name: Option<String>) -> Reply {
        let name = name.filter(|name| !name.is_empty());
        if self.settings.human_name == name {
            return Reply::already();
        }
        self.settings.human_name = name;
        self.settings_changed()
    }

    /// What messages call the human (`SetHumanName`), if anything does.
    pub(crate) fn human_name(&self) -> Option<&str> {
        self.settings.human_name.as_deref()
    }

    /// Asks for the agent in a pane to be compacted, at its next idle, empty prompt.
    fn compact(&mut self, compact: &pane_request::Compact) -> Reply {
        let Some(index) = self.pane_index(&compact.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", compact.pane));
        };
        let pane = &self.panes[index];
        let Some(agent) = pane.record.agent.clone() else {
            return Reply::refused(format!(
                "pane {} runs no agent Muster recognizes, so there is no context to compact",
                compact.pane
            ));
        };
        let focus = match typed_line(
            compact.focus.as_deref(),
            "a focus",
            "a longer brief belongs in a message (`muster msg post`)",
        ) {
            Ok(focus) => focus,
            Err(refused) => return refused,
        };
        let then = match typed_line(
            compact.then.as_deref(),
            "a prompt to carry on with",
            "a longer brief belongs in a file the prompt names",
        ) {
            Ok(then) => then,
            Err(refused) => return refused,
        };
        let Some(manifests) = self.detecting.manifests() else {
            return Reply::refused(
                "this daemon has not loaded its agents' manifests yet, so it cannot say how to \
                 compact one; ask again in a moment",
            );
        };
        let line = match manifests.session_compact(&muster_detect::Agent::new(&agent), focus) {
            None => {
                return Reply::refused(format!(
                    "{agent} gives Muster no way to compact it from its prompt, so pane {} is \
                     left as it is; `muster docs harnesses` says which harnesses can be",
                    compact.pane
                ));
            }
            Some(Err(why)) => return Reply::refused(why),
            Some(Ok(line)) => line,
        };
        log::info(
            "daemon.compact.asked",
            fields! {
                "pane" => compact.pane,
                "agent" => agent,
                "focused" => focus.is_some(),
                "then" => then.is_some(),
            },
        );
        self.panes[index].compaction.ask(line, then.map(str::to_string));
        self.compaction_changed(index);
        Reply::done()
    }

    /// Puts what pane `index` has queued to type at its agent's prompt on its record, and tells
    /// the subscribers when that changed.
    fn compaction_changed(&mut self, index: usize) {
        let pane = &mut self.panes[index];
        let pending = match pane.compaction.pending() {
            Pending::Nothing => proto::PendingCompaction::None,
            Pending::Compaction => proto::PendingCompaction::Queued,
            Pending::FollowUp => proto::PendingCompaction::FollowUp,
        };
        if replaced(&mut pane.record.pending_compaction, i32::from(pending)) {
            let record = pane.record.clone();
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
    }

    /// Checks a restart of a pane's agent, and hands back what runs it.
    fn restart(&self, restart: &pane_request::Restart) -> Result<Restarting, Reply> {
        let Some(index) = self.pane_index(&restart.pane) else {
            return Err(Reply::not_there(format!("no pane {} on this daemon", restart.pane)));
        };
        let pane = &self.panes[index];
        let name = &restart.pane;
        let Some(session) = pane
            .session_id
            .clone()
            .or_else(|| pane.resume.as_ref().map(|resume| resume.session.clone()))
        else {
            return Err(Reply::refused(Unrestartable::NoSession.said(name, "its agent")));
        };
        let agent = pane.record.agent.clone().unwrap_or_else(|| "its agent".to_string());
        let Some(resume) = &pane.resume else {
            return Err(Reply::refused(Unrestartable::NoResume.said(name, &agent)));
        };
        let (Some(group), Some(shell)) = (pane.io.agent_group(), pane.io.shell()) else {
            return Err(Reply::refused(Unrestartable::GroupUnknown.said(name, &agent)));
        };
        let group = group.cast_signed();
        if group == shell {
            return Err(Reply::refused(Unrestartable::PanesOwn.said(name, &agent)));
        }
        if pane.io.discards().is_restarting() {
            return Err(Reply::refused(format!("pane {name}'s agent is already restarting")));
        }
        let mut variables: Vec<(&str, &str)> =
            restart.env.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        variables.sort_unstable();
        if let Some(fault) =
            variables.iter().find_map(|&(name, value)| restart::variable_fault(name, value))
        {
            return Err(Reply::refused(fault));
        }
        let shell_path = self.settings.shell.as_ref().and_then(|shell| shell.command.as_deref());
        let shell_path = pty::shell(shell_path, &self.inherited);
        let command = spawn::command_line(&shell_path, &resume.command);
        let line = spawn::with_variables(&shell_path, &variables, &command);
        log::info(
            "daemon.pane.restarting",
            fields! {
                "pane" => name,
                "agent" => agent,
                "session" => session,
                "flags" => flag_names(resume.command.get(1..).unwrap_or_default()),
                "env" => variables.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(","),
            },
        );
        Ok(Restarting {
            pane: name.clone(),
            program: agent,
            session,
            line,
            target: restart::Live { io: Arc::clone(&pane.io), group, shell },
        })
    }

    /// Whether any line is still to be typed at an agent's prompt - a pane's name for its
    /// session, or a compaction - which keeps the doorbell's thread looking.
    pub(crate) fn wants_typing(&self) -> bool {
        self.wants_session_names()
            || self.panes.iter().any(|pane| pane.compaction.pending() != Pending::Nothing)
    }

    /// A line of the compaction in `pane` was typed into its agent and taken.
    pub(crate) fn compaction_taken(&mut self, pane: &str, line: &str) {
        if let Some(index) = self.pane_index(pane) {
            self.panes[index].compaction.taken(line);
            self.compaction_changed(index);
        }
    }

    /// Typing a line of the compaction in `pane` was given up; says the prompt that went with it,
    /// if a compaction given up had one.
    pub(crate) fn compaction_given_up(&mut self, pane: &str, line: &str) -> Option<String> {
        let index = self.pane_index(pane)?;
        let dropped = self.panes[index].compaction.given_up(line);
        self.compaction_changed(index);
        dropped
    }

    /// Whether a pane's name is typed into its agent's session (`SetNameSessions`).
    pub(crate) fn names_sessions(&self) -> bool {
        self.settings.name_sessions.unwrap_or(true)
    }

    fn set_scroll_multiplier(&mut self, multiplier: f64) -> Reply {
        if !multiplier.is_finite() || multiplier <= 0.0 {
            return Reply::refused(format!(
                "a scroll multiplier is finite and above zero, and this one is {multiplier}"
            ));
        }
        if self.settings.scroll_multiplier.unwrap_or(1.0).to_bits() == multiplier.to_bits() {
            return Reply::already();
        }
        self.settings.scroll_multiplier = Some(multiplier);
        self.resettle();
        self.settings_changed()
    }

    fn set_cursor(&mut self, set: proto::SetCursor) -> Reply {
        let Some(cursor) = set.cursor else {
            return Reply::refused("the request carries no cursor");
        };
        if self.settings.cursor.as_ref() == Some(&cursor) {
            return Reply::already();
        }
        self.settings.cursor = Some(cursor);
        self.settings_changed()
    }

    fn settings_changed(&mut self) -> Reply {
        let settings = self.settings.clone();
        self.emit(Payload::SettingsChanged(proto::SettingsChanged { settings: Some(settings) }));
        Reply::done()
    }

    /// Has the manifests loaded again with the app's, with the session unlocked
    /// ([`Loading::load`]), then finished by [`Session::manifests_loaded`].
    pub(crate) fn send_manifests(&mut self, sent: proto::SendManifests) -> Handled {
        self.manifest_loads += 1;
        Handled::Manifests(Box::new(Loading {
            detecting: Arc::clone(&self.detecting),
            app: sent
                .manifests
                .into_iter()
                .map(|manifest| (manifest.agent, manifest.toml))
                .collect(),
            load: self.manifest_loads,
        }))
    }

    /// Puts loaded manifests in use, and starts detection over in the panes whose agent is
    /// detected differently now. A pane whose agent's manifest is unchanged keeps its state:
    /// the app sends its manifests on every connect, and starting every pane over would publish
    /// each working agent unknown through a startup grace, then working again.
    pub(crate) fn manifests_loaded(&mut self, loading: Loading, loaded: Loaded) -> Reply {
        if loading.load < self.manifests_adopted {
            // A later send already put its manifests in use, and they supersede these: nothing
            // this send asked for was put in use.
            return Reply::already();
        }
        self.manifests_adopted = loading.load;
        let changed = self.detecting.adopt(loaded);
        self.say_which_agents_compact();
        let unchanged = changed.is_empty() && loading.app == self.app_manifests;
        self.app_manifests = loading.app;
        if unchanged {
            return Reply::already();
        }
        for pane in &self.panes {
            // A pane with no agent may be running one that only a new manifest names, and its
            // foreground may never change to prompt another probe. Starting it over costs it
            // nothing, since it publishes only what differs.
            let reset = match &pane.record.agent {
                Some(agent) => changed.iter().any(|changed| changed.id() == agent),
                None => !changed.is_empty(),
            };
            if reset {
                pane.io.reset_detection();
            }
        }
        Reply::done()
    }

    /// Says again of every pane whether its agent compacts. An override can give an agent a
    /// compact line or take one away while detection still finds the same agent, and a pane taken
    /// over from an older daemon arrives saying nothing of it; the app sends its manifests on
    /// every connect, which brings both here.
    fn say_which_agents_compact(&mut self) {
        let manifests = self.detecting.manifests();
        let mut changed = Vec::new();
        for pane in &mut self.panes {
            let compacts = agent_compacts(manifests.as_deref(), pane.record.agent.as_deref());
            if pane.record.agent_compacts != compacts {
                pane.record.agent_compacts = compacts;
                changed.push(pane.record.clone());
            }
        }
        for record in changed {
            self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Persistence

    /// What a restart needs of the session now, unless the daemon has begun to stop: then the
    /// session is being closed, and what it held was handed to the persister first
    /// ([`Session::close_everything`]).
    pub(crate) fn persisted_unless_stopping(&self) -> Option<persist::State> {
        (!self.stopping).then(|| self.persisted())
    }

    /// What a restart needs of the session now.
    pub(crate) fn persisted(&self) -> persist::State {
        persist::State {
            version: persist::VERSION,
            daemon: env!("CARGO_PKG_VERSION").to_string(),
            settings: self.settings.clone(),
            tabs: self
                .tabs
                .iter()
                .map(|tab| persist::Tab {
                    name: tab.name.clone(),
                    label: tab.label.clone(),
                    zoomed: tab.zoomed.clone(),
                    root: tab.root.clone(),
                    hidden: tab.hidden,
                })
                .collect(),
            panes: self
                .panes
                .iter()
                .map(|pane| persist::Pane {
                    name: pane.record.pane.clone(),
                    label: pane.record.label.clone(),
                    word: pane.record.word.clone(),
                    cwd: PathBuf::from(&pane.record.cwd),
                    grid: pane.io.grid(),
                    resume: pane.resume.clone(),
                    quiet: pane.record.quiet,
                    maker: pane.record.maker.clone(),
                    maker_closed: pane.record.maker_closed,
                })
                .collect(),
        }
    }

    /// Checks a saved tab against what is here and reserves its names, for its panes' shells to
    /// start with the session unlocked ([`Restoring::start`]) and [`Session::restored`] to
    /// finish. A tab or pane whose name is already here - a client was quicker - is skipped.
    fn prepare_restore(
        &mut self,
        tab: persist::Tab,
        saved: &HashMap<String, persist::Pane>,
        lost: &mut Lost,
    ) -> Option<Restoring> {
        if self.stopping {
            return None;
        }
        let taken = |session: &Session, name: &str| {
            session.reserved.contains(name)
                || session.pane_index(name).is_some()
                || session.tab_index(name).is_some()
        };
        if taken(self, &tab.name) {
            log::warn(
                "daemon.state.tab_taken",
                fields! {
                    "tab" => tab.name,
                    "impact" => "the saved tab is not brought back, since a client made one \
                                 of that name first",
                },
            );
            let panes = tab.root.panes();
            lost.panes.extend(
                panes.into_iter().filter(|pane| saved.contains_key(*pane)).map(str::to_string),
            );
            lost.tabs.push(tab.name);
            return None;
        }
        let mut panes = Vec::new();
        for name in tab.root.panes() {
            let Some(pane) = saved.get(name) else { continue };
            if taken(self, name) {
                log::warn(
                    "daemon.state.pane_taken",
                    fields! {
                        "pane" => name,
                        "impact" => "the saved pane is not brought back, since a client made \
                                     one of that name first",
                    },
                );
                lost.panes.push(name.to_string());
                continue;
            }
            // The word it had, unless a client's pane took it first; a state file from before
            // words has none, and gets one now.
            let mut pane = pane.clone();
            let word = pane
                .word
                .take()
                .filter(|word| self.called(word, name).is_none())
                .unwrap_or_else(|| self.draw_word(given_name(pane.label.as_deref())));
            self.naming.insert(word.clone(), name.to_string());
            pane.word = Some(word.clone());
            let called = given_name(pane.label.as_deref()).unwrap_or(&word).to_string();
            let which = spawn::Which { pane: name, name: &called };
            let shell = self.settings.shell.clone().unwrap_or_default();
            let resume = pane.resume.clone().filter(|_| self.resumes_agents());
            let command = resume.as_ref().map(|resume| {
                spawn::command_line(
                    &pty::shell(shell.command.as_deref(), &self.inherited),
                    &resume.command,
                )
            });
            let launch = self.launch_with(&shell, which, command.as_deref(), &HashMap::new());
            let plain = self.launch_with(&shell, which, None, &HashMap::new());
            let fallback = proto::Shell { command: None, ..shell };
            let fallback = self.launch_with(&fallback, which, None, &HashMap::new());
            panes.push(RestoringPane { saved: pane, resume, launch, plain, fallback });
        }
        self.reserved.insert(tab.name.clone());
        for pane in &panes {
            self.reserved.insert(pane.saved.name.clone());
        }
        Some(Restoring { tab, panes, home: self.home.clone() })
    }

    /// Finishes restoring a tab once its panes' shells have started, or failed to. The tab
    /// comes back with the panes that did.
    fn restored(&mut self, restoring: Restoring, started: Vec<Restarted>, lost: &mut Lost) {
        let Restoring { tab, panes, .. } = restoring;
        self.reserved.remove(&tab.name);
        for pane in &panes {
            self.reserved.remove(&pane.saved.name);
            self.naming.retain(|_, restoring| *restoring != pane.saved.name);
        }
        let mut back = HashSet::new();
        for (pane, Restarted { cwd, program, started, resumed }) in panes.into_iter().zip(started) {
            let program = &program;
            let name = pane.saved.name;
            let (master, child) = match started {
                Ok(started) => started,
                Err(error) => {
                    Self::could_not_start(&name, program, &cwd, &error);
                    lost.panes.push(name);
                    continue;
                }
            };
            if self.stopping {
                drop(master);
                pty::abandon(child.id().cast_signed());
                continue;
            }
            let resume = pane.resume.filter(|_| resumed);
            let record = proto::Pane {
                pane: name.clone(),
                label: pane.saved.label,
                word: pane.saved.word,
                cwd: cwd.display().to_string(),
                command: resume.as_ref().map(|resume| resume.command.join(" ")),
                quiet: pane.saved.quiet,
                maker: pane.saved.maker,
                maker_closed: pane.saved.maker_closed,
                session_id: resume.as_ref().map(|resume| resume.session.clone()),
                resume: resume.as_ref().map(persist::Resume::record),
                ..proto::Pane::default()
            };
            if self.open(record, pane.saved.grid, master, child, program).is_ok() {
                if let Some(resume) = resume {
                    Self::resumed(&name, &resume);
                    let index = self.pane_index(&name).expect("the pane just opened");
                    self.panes[index].session_id = Some(resume.session.clone());
                    self.panes[index].resume = Some(resume);
                }
                self.restored_from_file = true;
                back.insert(name);
            } else {
                lost.panes.push(name);
            }
        }
        if self.stopping {
            return;
        }
        let gone: Vec<String> = tab
            .root
            .panes()
            .into_iter()
            .filter(|pane| !back.contains(*pane))
            .map(str::to_string)
            .collect();
        let mut root = Some(tab.root);
        for pane in &gone {
            root = root.and_then(|root| root.without(pane).0);
        }
        let Some(root) = root else {
            log::warn(
                "daemon.state.tab_lost",
                fields! {
                    "tab" => tab.name,
                    "impact" => "none of the saved tab's panes started, so it is not brought back",
                    "check" => "the daemon.pane.not_started records before this one",
                },
            );
            lost.tabs.push(tab.name);
            return;
        };
        let zoomed = tab.zoomed.filter(|pane| root.contains(pane));
        let tab = Tab { name: tab.name, label: tab.label, root, zoomed, hidden: tab.hidden };
        let record = tab.record();
        self.tabs.push(tab);
        self.emit(Payload::TabOpened(proto::TabOpened { tab: Some(record) }));
    }

    /// Says which pane came back running its agent's session: the program, the session, and
    /// which flags went along by name. Never a flag's value or the command as typed - a carried
    /// `--settings` or `--append-system-prompt` can hold a secret, and what somebody typed stays
    /// out of the run log. The pane's own record carries the whole command, for its owner.
    fn resumed(pane: &str, resume: &persist::Resume) {
        let (program, arguments) = resume.command.split_first().unzip();
        let arguments = arguments.unwrap_or_default();
        log::info(
            "daemon.state.resumed",
            fields! {
                "pane" => pane,
                "agent" => resume.agent,
                "session" => resume.session,
                "program" => program.map(String::as_str).unwrap_or_default(),
                "arguments" => arguments.len(),
                "flags" => flag_names(arguments),
                "carried" => resume.uncarried.is_none(),
            },
        );
    }

    // -----------------------------------------------------------------------------------------
    // Handoff (MIP-3 section 10)

    /// Checks the daemon can be replaced now, and names the program to replace it with. The
    /// handoff runs with the session unlocked ([`crate::handoff::hand_over`]), and marks the
    /// session with [`Session::begin_replacing`] once the program has answered.
    fn replace(&mut self, replace: session_request::Replace) -> Handled {
        if let Some(why) = self.cannot_replace() {
            return Handled::Reply(Reply::refused(why));
        }
        let Some(program) = replace.program.map(PathBuf::from).or_else(|| self.executable.clone())
        else {
            return Handled::Reply(Reply::refused(
                "this daemon could not find its own executable; name the program to replace it with",
            ));
        };
        Handled::Replace(Box::new(Replacement { program, data: replace.data.map(PathBuf::from) }))
    }

    /// Checks again that the daemon can be replaced, since the session was unlocked while the
    /// program was asked its version, and marks it as being replaced: from here a request that
    /// changes anything is refused, so what is handed over is what the session holds.
    pub(crate) fn begin_replacing(&mut self) -> Result<(), &'static str> {
        if let Some(why) = self.cannot_replace() {
            return Err(why);
        }
        self.replacing = Replacing::Underway;
        Ok(())
    }

    fn cannot_replace(&self) -> Option<&'static str> {
        if self.stopping {
            Some("the daemon is stopping")
        } else if self.restoring {
            Some("the daemon is still bringing back its saved tabs; ask again once it has")
        } else if self.replacing != Replacing::No {
            Some("the daemon is already being replaced")
        } else {
            None
        }
    }

    pub(crate) fn log(&self) -> Option<Arc<DaemonLog>> {
        self.log.clone()
    }

    /// What a handoff sends.
    pub(crate) fn handing(&self) -> Handing {
        Handing {
            state: self.persisted(),
            app_manifests: self.app_manifests.clone(),
            panes: self
                .panes
                .iter()
                .map(|pane| HandedPane {
                    record: pane.record.clone(),
                    process: pane.process(),
                    io: Arc::clone(&pane.io),
                    turns: pane.turns,
                    session_id: pane.session_id.clone(),
                })
                .collect(),
            persister: Arc::clone(&self.persister),
            log: self.log.clone(),
        }
    }

    /// Takes what [`Session::handing`] captured again, once every pane's reader is held and
    /// what they reported before is applied: nothing about a pane changes after this. Fails
    /// when the panes are no longer the ones captured: a pane that ended since is in no tab of
    /// the state, and one made since was never held.
    pub(crate) fn recapture(&self, handing: &mut Handing) -> Result<(), String> {
        for handed in &mut handing.panes {
            let Some(pane) = self.panes.iter().find(|pane| Arc::ptr_eq(&pane.io, &handed.io))
            else {
                return Err(format!("pane {} ended during the handoff", handed.record.pane));
            };
            handed.record = pane.record.clone();
        }
        if let Some(made) = self
            .panes
            .iter()
            .find(|pane| !handing.panes.iter().any(|handed| Arc::ptr_eq(&pane.io, &handed.io)))
        {
            return Err(format!("pane {} was made during the handoff", made.record.pane));
        }
        handing.state = self.persisted();
        Ok(())
    }

    pub(crate) fn reports(&self) -> Reports {
        self.reports.clone()
    }

    /// The new daemon serves: subscribers are told, and are handed back to have what they were
    /// sent written before the daemon exits.
    pub(crate) fn replaced(&mut self, pid: u32, daemon_version: String) -> Vec<Outbox> {
        self.replacing = Replacing::HandedOff;
        self.emit(Payload::Replaced(proto::Replaced { pid, daemon_version }));
        self.subscribers.clone()
    }

    /// The handoff failed, and this daemon goes on as it was. True when a stop was asked for
    /// while it ran, which is the caller's to carry out now.
    pub(crate) fn not_replaced(&mut self) -> bool {
        self.replacing = Replacing::No;
        std::mem::take(&mut self.stop_deferred)
    }

    /// Stops the daemon unless a handoff has begun. Checked and done in one hold, so a handoff
    /// cannot begin in between: it is refused once the daemon is stopping. While one is under
    /// way, closing a pane would end a process the new daemon may already hold.
    pub(crate) fn stop_unless_replacing(&mut self) -> Stopping {
        match self.replacing {
            Replacing::No => {
                self.close_everything();
                Stopping::Now
            }
            Replacing::Underway => {
                if !self.stop_deferred {
                    log::info(
                        "daemon.stop.deferred",
                        fields! {
                            "why" => "a handoff is under way",
                            "impact" => "if it succeeds this daemon exits and the new one serves \
                                         every pane; if it fails this daemon stops as asked",
                        },
                    );
                }
                self.stop_deferred = true;
                Stopping::Deferred
            }
            Replacing::HandedOff => Stopping::HandedOff,
        }
    }

    /// Takes over a pane from the daemon this one replaces. Its terminal is rebuilt from
    /// `replay` with whatever parsing it asked for thrown away: a replay can provoke a reply of
    /// its own, and none of that belongs on the pane's input. Its reader starts held, and reads
    /// nothing until [`Session::release_readers`].
    pub(crate) fn adopt(
        &mut self,
        mut record: proto::Pane,
        grid: Grid,
        master: OwnedFd,
        process: Option<i32>,
        replay: &[u8],
        resuming: Resuming<'_>,
    ) -> Result<(), String> {
        let mut screen = Screen::new(grid, &self.settled).map_err(|error| error.to_string())?;
        drop(screen.output(replay));
        screen.replayed_from(resuming.oldest_row);
        // A daemon from before words hands its panes over with none.
        if record.word.is_none() {
            record.word = Some(self.draw_word(given_name(record.label.as_deref())));
        }
        self.next_serial += 1;
        let watching = Watching {
            ended: &self.ended,
            reports: &self.reports,
            host: &self.host,
            detecting: &self.detecting,
            persister: &self.persister,
            held: true,
            detection: resuming.detection,
            turns: resuming.turns,
        };
        let name = record.pane.clone();
        let mut pane = Pane::start(
            record,
            self.next_serial,
            master,
            screen,
            grid,
            process.map(Process::Adopted),
            &watching,
        )
        .map_err(|error| format!("pane {name}: {error}"))?;
        pane.session_id = resuming.session_id.map(str::to_string);
        pane.resume = resuming.resume.cloned();
        pane.describe_session();
        self.panes.push(pane);
        Ok(())
    }

    /// The tabs of a handed-over state, every pane in them already adopted and every adopted
    /// pane in one of them: a pane in no tab could never be closed or saved.
    pub(crate) fn adopt_tabs(&mut self, tabs: Vec<persist::Tab>) -> Result<(), String> {
        if let Some(stray) =
            in_no_tab(self.panes.iter().map(|pane| pane.record.pane.as_str()), &tabs)
        {
            return Err(format!("pane {stray} was handed over in no tab"));
        }
        for tab in tabs {
            if let Some(missing) =
                tab.root.panes().into_iter().find(|pane| self.pane_index(pane).is_none())
            {
                return Err(format!(
                    "tab {} holds pane {missing}, which was not handed over",
                    tab.name
                ));
            }
            self.tabs.push(Tab {
                name: tab.name,
                label: tab.label,
                root: tab.root,
                zoomed: tab.zoomed,
                hidden: tab.hidden,
            });
        }
        Ok(())
    }

    /// Lets every adopted pane's reader go on, once the handoff has committed.
    pub(crate) fn release_readers(&self) {
        for pane in &self.panes {
            pane.io.release_reader();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Lookups

    /// What a stream or input connection needs of a pane, found by name.
    pub(crate) fn pane_io(&self, pane: &str) -> Option<Arc<PaneIo>> {
        self.pane_index(pane).map(|index| Arc::clone(&self.panes[index].io))
    }

    /// Something of every pane, read with the session held, for work that goes on without it -
    /// the message service's presence (MIP-4, section 7).
    pub(crate) fn each_pane<T>(&self, view: impl Fn(&Pane) -> T) -> Vec<T> {
        self.panes.iter().map(view).collect()
    }

    /// Whether any pane here is somebody's helper, or a maker closed with helpers open (MIP-7),
    /// which a watch on helpers needs to look at before reading every pane.
    pub(crate) fn has_helpers(&self) -> bool {
        !self.orphans.is_empty() || self.panes.iter().any(|pane| pane.record.maker.is_some())
    }

    /// Whether any pane's name is still to be typed into its agent's session: one wanted, for an
    /// agent whose manifest says how. A named pane whose agent's harness cannot be renamed keeps
    /// its name wanted, and must not keep the doorbell's thread looking.
    pub(crate) fn wants_session_names(&self) -> bool {
        if !self.names_sessions() {
            return false;
        }
        let Some(manifests) = self.detecting.manifests() else { return false };
        self.panes.iter().any(|pane| {
            let (Some(name), Some(agent)) =
                (pane.session_name.wanted(), pane.record.agent.as_deref())
            else {
                return false;
            };
            manifests.session_rename(&muster_detect::Agent::new(agent), name).is_some()
        })
    }

    /// `name` was typed into the session of the agent in `pane`, or typing it was given up.
    pub(crate) fn session_name_typed(&mut self, pane: &str, name: &str) {
        if let Some(index) = self.pane_index(pane) {
            self.panes[index].session_name.typed(name);
        }
    }

    fn pane_index(&self, pane: &str) -> Option<usize> {
        self.panes.iter().position(|candidate| candidate.record.pane == pane)
    }

    /// The pane other than `except` that `name` belongs to here: one called it, or one drawn it
    /// as its word and named something else since.
    ///
    /// A renamed pane keeps its word, and the agent list shows it beside the given name, so the
    /// word stays that pane's: drawn again for another pane, two rows would show it and a name
    /// typed from one of them would reach the other.
    fn called(&self, name: &str, except: &str) -> Option<&str> {
        self.answers_to(name, except).or_else(|| self.drawn(name, except))
    }

    /// The pane other than `except` whose name is `name` here: one open, or one starting.
    fn answers_to(&self, name: &str, except: &str) -> Option<&str> {
        self.panes
            .iter()
            .map(|pane| &pane.record)
            .find(|record| record.pane != except && record.name() == Some(name))
            .map(|record| record.pane.as_str())
            .or_else(|| self.naming.get(name).filter(|pane| *pane != except).map(String::as_str))
    }

    /// The pane other than `except` that was drawn `word`, whatever it is called now.
    fn drawn(&self, word: &str, except: &str) -> Option<&str> {
        self.panes
            .iter()
            .map(|pane| &pane.record)
            .find(|record| record.pane != except && record.word.as_deref() == Some(word))
            .map(|record| record.pane.as_str())
    }

    /// Why a pane cannot be given `name`, when another pane here holds it.
    fn name_taken(&self, name: &str, except: &str) -> Option<Reply> {
        if let Some(holder) = self.answers_to(name, except) {
            return Some(name_held(name, holder));
        }
        self.drawn(name, except).map(|holder| word_held(name, holder))
    }

    /// A word for a pane that no pane here is called, nor `besides`, the name it is being given,
    /// nor an agent here joined messaging by: a post to that word would mean both.
    fn draw_word(&self, besides: Option<&str>) -> String {
        let joined = poison::lock(&self.joined, "daemon.messages.joined");
        muster_core::words::draw(|word| {
            besides == Some(word) || self.called(word, "").is_some() || joined.contains_key(word)
        })
    }

    /// The live agent that joined messaging by `name`, other than one in `naming`, described for
    /// a refusal: a pane given that name would make a post to it mean two agents. Live as a post
    /// judges it (`muster_msg`'s `pane_called`): its pane is open here, or, with no pane, it has
    /// not left and its inbox answered when [`Probed`] asked, before this lock was taken.
    fn joined_by(&self, name: &str, naming: &str) -> Option<String> {
        let known = poison::lock(&self.joined, "daemon.messages.joined").get(name)?.clone();
        match known.pane.as_deref() {
            Some(pane) if pane == naming => None,
            Some(pane) => self.pane_index(pane).map(|_| format!("{name} (pane {pane})")),
            None if !known.gone && self.probed.answers(&known) => {
                let at = known.inbox.as_ref().map_or("", |inbox| inbox.socket.as_str());
                Some(format!("{name} (the session at {at})"))
            }
            None => None,
        }
    }

    fn tab_index(&self, tab: &str) -> Option<usize> {
        self.tabs.iter().position(|candidate| candidate.name == tab)
    }

    fn tab_of(&self, pane: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.root.contains(pane))
    }
}

/// The history each pane keeps, in bytes.
fn scrollback(settings: &proto::Settings) -> usize {
    settings
        .scrollback_bytes
        .map_or(screen::DEFAULT_SCROLLBACK, |bytes| usize::try_from(bytes).unwrap_or(usize::MAX))
}

/// The name somebody gave a pane, as its record reads it (`proto::Pane::name`): blank is none.
fn given_name(label: Option<&str>) -> Option<&str> {
    label.map(str::trim).filter(|label| !label.is_empty())
}

/// Whether the inboxes of agents a request would name a pane after answered, asked before the
/// session's lock is taken: an inbox is asked by connecting, which waits up to a second on a
/// session that has stopped reading, and every request here would wait with it.
#[derive(Debug, Default)]
pub(crate) struct Probed(Vec<(Option<muster_msg::Inbox>, bool)>);

impl Probed {
    /// Asks the inbox of the agent with no pane that joined by the name `service` would give a
    /// pane - a new pane's, a rename's, or a session's its pane would follow - if there is one.
    pub(crate) fn before(joined: &crate::messages::Joined, service: Option<&Service>) -> Probed {
        let name = match service {
            Some(Service::Pane(proto::PaneRequest { request: Some(request) })) => match request {
                pane_request::Request::Create(create) => create.label.as_deref(),
                pane_request::Request::Rename(rename) => rename.label.as_deref(),
                pane_request::Request::Report(report) => report.session_name.as_deref(),
                _ => None,
            },
            _ => None,
        };
        let Some(name) = given_name(name) else { return Probed::default() };
        // Copied out, so the probe never holds up messaging's updates to the set.
        let known = poison::lock(joined, "daemon.messages.joined").get(name).cloned();
        let Some(known) = known.filter(|known| known.pane.is_none() && !known.gone) else {
            return Probed::default();
        };
        let answered = crate::messages::inbox_answers(&known);
        Probed(vec![(known.inbox, answered)])
    }

    /// Whether `participant`'s inbox answered. One that was not asked - it joined by the name
    /// after the probe, or with another inbox - counts as answering: refusing a name that turns
    /// out free costs a retry, and giving one that is not makes a post to it mean two agents.
    fn answers(&self, participant: &muster_msg::Participant) -> bool {
        self.0
            .iter()
            .find(|(inbox, _)| *inbox == participant.inbox)
            .is_none_or(|(_, answered)| *answered)
    }
}

/// A pane cannot be called what an agent here joined messaging by while that agent is live,
/// because `--to` the name would then mean both.
fn name_joined(name: &str, who: &str) -> Reply {
    Reply::refused(format!(
        "{name:?} is the name {who} joined messaging by, and a post to it would then mean two \
         agents; choose another name, or have that agent join under another"
    ))
}

/// A pane cannot be called what another pane here is, because a name says which pane it is.
fn name_held(name: &str, holder: &str) -> Reply {
    Reply::refused(format!(
        "pane {holder} on this daemon is already called {name:?}, and two panes cannot share a \
         name, since a name says which pane it is; choose another, or rename {holder} first"
    ))
}

fn word_held(word: &str, holder: &str) -> Reply {
    Reply::refused(format!(
        "{word:?} is the word pane {holder} on this daemon was drawn, which it keeps after a \
         rename and the agent list shows beside its name; choose another name"
    ))
}

/// Muster's names are minted as short ASCII words (`p1w3r07bsd`). The daemon holds any name to
/// that shape, since it becomes an environment variable's value and, later, a line in a file.
pub(crate) fn valid_name(what: &str, name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(format!(
            "{name:?} is not a {what} name: one to 64 printable ASCII characters, no spaces"
        ));
    }
    Ok(())
}

pub(crate) fn valid_ratio(ratio: f32) -> Result<f32, String> {
    if ratio.is_finite() && ratio > 0.0 && ratio < 1.0 {
        Ok(ratio)
    } else {
        Err(format!("a ratio is strictly between 0 and 1, and {ratio} is not"))
    }
}

pub(crate) fn grid(grid: proto::Grid) -> Result<Grid, String> {
    let cells = |count: u32| u16::try_from(count).ok().filter(|&count| count > 0);
    let pixels = |count: u32| u16::try_from(count).ok();
    match (cells(grid.cols), cells(grid.rows), pixels(grid.width_px), pixels(grid.height_px)) {
        (Some(cols), Some(rows), Some(width_px), Some(height_px)) => {
            Ok(Grid { cols, rows, width_px, height_px })
        }
        _ => Err(format!(
            "a grid of {}x{} cells over {}x{} pixels is not one a terminal can have",
            grid.cols, grid.rows, grid.width_px, grid.height_px
        )),
    }
}

fn side(side: i32) -> Option<tree::Side> {
    match proto::Side::try_from(side).ok()? {
        proto::Side::Left => Some(tree::Side::Left),
        proto::Side::Right => Some(tree::Side::Right),
        proto::Side::Up => Some(tree::Side::Up),
        proto::Side::Down => Some(tree::Side::Down),
        proto::Side::Unspecified => None,
    }
}

fn branch(branch: i32) -> Option<tree::Branch> {
    match proto::Branch::try_from(branch).ok()? {
        proto::Branch::First => Some(tree::Branch::First),
        proto::Branch::Second => Some(tree::Branch::Second),
        proto::Branch::Unspecified => None,
    }
}

fn node_record(node: &Node) -> proto::Node {
    let node = match node {
        Node::Pane(name) => proto::node::Node::Pane(name.clone()),
        Node::Split { axis, ratio, first, second } => {
            proto::node::Node::Split(Box::new(proto::Split {
                axis: match axis {
                    tree::Axis::Columns => proto::Axis::Columns,
                    tree::Axis::Rows => proto::Axis::Rows,
                }
                .into(),
                ratio: *ratio,
                first: Some(Box::new(node_record(first))),
                second: Some(Box::new(node_record(second))),
            }))
        }
    };
    proto::Node { node: Some(node) }
}

/// Whether `agent` can be compacted from its prompt, by the manifests in use.
fn agent_compacts(manifests: Option<&Manifests>, agent: Option<&str>) -> bool {
    manifests
        .zip(agent)
        .is_some_and(|(manifests, agent)| manifests.compacts(&muster_detect::Agent::new(agent)))
}

fn turn_ended(pane: &str, agent: Option<&str>, waiting: bool) {
    log::debug(
        "daemon.agent.turn_ended",
        fields! { "pane" => pane, "agent" => agent.unwrap_or_default(), "waiting" => waiting },
    );
}

/// Says once, when it starts, that detection's rules have stopped reading a pane's agent.
fn unreadable_warning(pane: &str, agent: Option<&str>) {
    log::warn(
        "daemon.detection.unreadable",
        fields! {
            "pane" => pane,
            "agent" => agent.unwrap_or_default(),
            "why" => "for a minute the screen kept changing while no rule read it, or the rules \
                      read idle while the agent reported working",
            "impact" => "the pane's state comes only from the agent's own reports; without them \
                         it reads idle while the agent may be working",
            "check" => "whether the harness was updated past what its manifest knows (compare its \
                        version with the manifest's), the manifest in \
                        ~/.muster/agent-detection/, and whether the harness's hooks are installed",
        },
    );
}

/// `text` trimmed, or none when it is blank, when it can be typed at an agent's prompt as one
/// line; refused otherwise, `what` naming it and `longer` saying where a longer one belongs.
fn typed_line<'a>(
    text: Option<&'a str>,
    what: &str,
    longer: &str,
) -> Result<Option<&'a str>, Reply> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.chars().any(char::is_control) {
        return Err(Reply::refused(format!(
            "{what} is typed at the agent's prompt as one line, so it cannot hold a newline or \
             any other control character"
        )));
    }
    if text.len() > FOCUS_BYTES {
        return Err(Reply::refused(format!(
            "{what} is at most {FOCUS_BYTES} bytes, and this one is {}; {longer}",
            text.len()
        )));
    }
    Ok(Some(text))
}

/// Sets `field` to `value`, saying whether that changed it.
fn replaced<T: PartialEq>(field: &mut T, value: T) -> bool {
    if *field == value {
        return false;
    }
    *field = value;
    true
}

/// Whether a pane's agent has finished something nobody has seen, once its detection goes from
/// `before` to `after`. An agent that was working or waiting on you has finished when it goes
/// idle or leaves the pane, which is how a crash or a one-shot run ends; working or waiting
/// again is new work, which a view shows as that instead. An idle agent leaving changes nothing.
fn finished_unseen(
    before: (Option<&str>, proto::AgentState),
    after: (Option<&str>, proto::AgentState),
    was: bool,
) -> bool {
    use proto::AgentState::{Blocked, Idle, Working};
    let busy = |state| matches!(state, Working | Blocked);
    if after.0.is_some() && busy(after.1) {
        return false;
    }
    let stopped = after.1 == Idle || after.0 != before.0;
    was || (before.0.is_some() && busy(before.1) && stopped)
}

/// Where a change of agent state leaves the agent's turn: working or waiting on you after not
/// doing either is a turn started, and idle after either is one ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    Started,
    Ended,
    Neither,
}

impl Turn {
    pub(crate) fn between(before: proto::AgentState, after: proto::AgentState) -> Turn {
        use proto::AgentState::{Blocked, Idle, Working};
        let busy = |state| matches!(state, Working | Blocked);
        match (busy(before), busy(after)) {
            (false, true) => Turn::Started,
            (true, false) if after == Idle => Turn::Ended,
            _ => Turn::Neither,
        }
    }
}

/// Marks the pane's agent as reporting through its adapter, saying whether that is news. Only once
/// detection has named the agent: a report from no agent - a statusline's last run after its agent
/// left - would otherwise pass to whichever agent comes next. One naming another agent than the
/// detected one is not this agent's adapter either.
fn adapter_heard(record: &mut proto::Pane, agent: &str) -> bool {
    let own = record.agent.as_deref().is_some_and(|detected| agent.is_empty() || detected == agent);
    let heard = own && record.adapter() != proto::Adapter::Reporting;
    if heard {
        record.set_adapter(proto::Adapter::Reporting);
    }
    heard
}

fn is_waiting(facts: Option<&proto::AgentFacts>) -> bool {
    facts.is_some_and(|facts| facts.waiting.is_some())
}

/// Forgets what was to be typed for an agent that has left its pane, saying so when anything was.
fn forget_compaction(compaction: &mut Compaction, pane: &str, agent: &str, restarting: bool) {
    let pending = compaction.pending();
    if pending != Pending::Nothing {
        log::warn(
            "daemon.compact.dropped",
            fields! {
                "pane" => pane,
                "agent" => agent,
                "follow_up" => pending == Pending::FollowUp,
                "restarting" => restarting,
                "impact" => "the compaction, or the prompt it was to be followed by, is not typed \
                             into the agent that comes next, even one resuming the same session",
                "check" => "whether the next agent still needs it; `muster pane compact` asks \
                            again",
            },
        );
    }
    compaction.cleared();
}

/// At the end of an agent's turn, forgets what it was waiting on unless it said so again during
/// that turn: its work has woken it and it has finished, or the person moved it on. A wait its
/// screen shows is the screen's to end. Says whether the wait was forgotten.
fn settle_wait(record: &mut proto::Pane, turns: &mut Turns, by: &str) -> bool {
    if std::mem::replace(&mut turns.wait_declared, false) || turns.wait_on_screen {
        return false;
    }
    let Some(facts) = record.facts.as_mut() else { return false };
    if facts.waiting.take().is_none() {
        return false;
    }
    if *facts == proto::AgentFacts::default() {
        record.facts = None;
    }
    log::debug("daemon.report.waiting_cleared", fields! { "pane" => record.pane, "by" => by });
    true
}

/// Takes what the agent's screen says it waits on while idle (`on_screen`), from a rule such as
/// Claude Code's done line counting background shells still running: the wait the agent would
/// have said itself, ended when the screen stops saying it. A wait the agent said is its own,
/// and the screen does not replace it.
fn wait_on_screen(record: &mut proto::Pane, turns: &mut Turns, on_screen: Option<String>) {
    let said = record.facts.as_ref().and_then(|facts| facts.waiting.as_ref());
    if said.is_some() && !turns.wait_on_screen {
        return;
    }
    if said == on_screen.as_ref() {
        return;
    }
    turns.wait_on_screen = on_screen.is_some();
    let facts = record.facts.get_or_insert_with(proto::AgentFacts::default);
    if let Some(on) = on_screen {
        log::debug("daemon.detect.waiting", fields! { "pane" => record.pane, "on" => on });
        facts.waiting = Some(on);
        return;
    }
    facts.waiting = None;
    if *facts == proto::AgentFacts::default() {
        record.facts = None;
    }
    log::debug(
        "daemon.report.waiting_cleared",
        fields! { "pane" => record.pane, "by" => "screen" },
    );
}

/// The first of `panes` that none of `tabs` holds.
fn in_no_tab<'a>(
    mut panes: impl Iterator<Item = &'a str>,
    tabs: &[persist::Tab],
) -> Option<&'a str> {
    panes.find(|pane| !tabs.iter().any(|tab| tab.root.panes().contains(pane)))
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use muster_daemon_proto::connection;

    use super::*;

    /// A session with no panes, persisting to `file`, whose events reach the returned stream.
    fn session(file: &Path) -> (Arc<Shared>, UnixStream) {
        let places = Places {
            home: PathBuf::from("/"),
            overrides: None,
            reachable: spawn::Reachable { daemon: None, socket: file.with_extension("sock") },
            executable: None,
            data: Data::unchecked(PathBuf::from("/nonexistent")),
            log: None,
        };
        let persister = Persister::new(file.to_path_buf(), false);
        let saved = Saved { persister, settings: None, restoring: true };
        let (stopping, _) = std::sync::mpsc::channel();
        let path = file.with_extension("sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let lock = std::fs::File::create(file.with_extension("lock")).unwrap();
        let socket = Socket::new(path, listener, lock).unwrap();
        let shared = Shared::new(1, stopping, Vec::new(), places, saved, socket);
        let (ours, theirs) = UnixStream::pair().unwrap();
        let outbox = Outbox::open(&ours).unwrap();
        shared.lock().subscribers.push(outbox);
        (shared, theirs)
    }

    fn next_event(stream: &mut UnixStream) -> Payload {
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let message = connection::receive::<proto::ControlMessage>(stream).unwrap().unwrap();
        match message.message {
            Some(proto::control_message::Message::Event(event)) => event.event.unwrap(),
            other => panic!("not an event: {other:?}"),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("muster-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An agent's inbox is asked before the session's lock is taken, and only the answer is read
    /// under it: one that was not asked, because it joined after the probe, counts as live.
    #[test]
    fn a_name_is_probed_before_the_lock_and_an_agent_not_probed_counts_as_live() {
        let joiner = |socket: &str| -> muster_msg::Participant {
            serde_json::from_value(serde_json::json!({
                "name": "critic",
                "inbox": { "socket": socket, "inode": 1 },
            }))
            .unwrap()
        };
        let gone = joiner("/nonexistent/critic.sock");
        let by_name: crate::messages::Joined =
            Arc::new(Mutex::new(BTreeMap::from([("critic".to_string(), gone.clone())])));
        let renaming = |label: &str| {
            Service::Pane(proto::PaneRequest {
                request: Some(pane_request::Request::Rename(pane_request::Rename {
                    pane: "p1".to_string(),
                    label: Some(label.to_string()),
                })),
            })
        };

        let probed = Probed::before(&by_name, Some(&renaming(" critic ")));
        assert!(!probed.answers(&gone), "an inbox that is not there does not answer");
        assert!(probed.answers(&joiner("/elsewhere/critic.sock")), "one not asked counts as live");
        let unasked = Probed::before(&by_name, Some(&renaming("builder")));
        assert!(unasked.answers(&gone), "a name nobody joined by asks no inbox");
    }

    #[test]
    fn an_agent_finishes_when_it_stops_working_or_waiting_by_going_idle_or_leaving() {
        use proto::AgentState::{Blocked, Idle, Unknown, Working};
        let claude = Some("claude");
        let finished = |before, after| finished_unseen(before, after, false);
        assert!(finished((claude, Working), (claude, Idle)), "a turn ends");
        assert!(finished((claude, Blocked), (claude, Idle)), "a prompt answered, then idle");
        assert!(finished((claude, Working), (None, Unknown)), "it ended while working");
        assert!(finished((claude, Blocked), (None, Unknown)), "it ended waiting on you");
        assert!(!finished((claude, Idle), (None, Unknown)), "an idle agent quit");
        assert!(!finished((claude, Working), (claude, Blocked)), "waiting on you is not done");
        assert!(!finished((None, Unknown), (claude, Idle)), "an agent appearing");
        assert!(!finished((claude, Idle), (claude, Working)));

        assert!(finished_unseen((claude, Idle), (None, Unknown), true), "kept until seen");
        assert!(!finished_unseen((claude, Idle), (claude, Working), true), "new work clears it");
        assert!(!finished_unseen((claude, Idle), (claude, Blocked), true), "so does a prompt");
        assert!(!finished_unseen((claude, Idle), (Some("codex"), Working), true));
    }

    /// What an idle agent's screen says it waits on is its wait until the screen stops saying
    /// it, whichever way the turn's end is heard; and a wait the agent said is its own.
    #[test]
    fn a_wait_its_screen_shows_lasts_as_long_as_the_screen_says_it() {
        let shells = || Some("its background shells".to_string());
        let waits = |record: &proto::Pane| record.facts.as_ref().and_then(|f| f.waiting.clone());
        let mut record = proto::Pane::default();
        let mut turns = Turns { wait_declared: false, ..Turns::default() };

        wait_on_screen(&mut record, &mut turns, shells());
        assert_eq!(waits(&record), shells());
        assert!(!settle_wait(&mut record, &mut turns, "reported turn end"), "the screen's to end");
        assert_eq!(waits(&record), shells());
        wait_on_screen(&mut record, &mut turns, None);
        assert_eq!(record.facts, None, "nothing else was said, so nothing is left");

        let said = Some("the full gate".to_string());
        record.facts = Some(proto::AgentFacts { waiting: said.clone(), ..Default::default() });
        turns.wait_on_screen = false;
        wait_on_screen(&mut record, &mut turns, shells());
        assert_eq!(waits(&record), said, "the agent's own word stands");
        wait_on_screen(&mut record, &mut turns, None);
        assert_eq!(waits(&record), said, "and the screen does not end it");
    }

    #[test]
    fn a_pane_handed_over_in_no_tab_is_found() {
        let tab = |name: &str, panes: [&str; 2]| persist::Tab {
            name: name.to_string(),
            label: proto::Label::default(),
            zoomed: None,
            root: Node::Split {
                axis: tree::Axis::Columns,
                ratio: 0.5,
                first: Box::new(Node::Pane(panes[0].to_string())),
                second: Box::new(Node::Pane(panes[1].to_string())),
            },
            hidden: false,
        };
        let tabs = [tab("t1", ["p1", "p2"]), tab("t2", ["p3", "p4"])];
        assert_eq!(in_no_tab(["p1", "p4"].into_iter(), &tabs), None);
        assert_eq!(in_no_tab(["p1", "p5", "p2"].into_iter(), &tabs), Some("p5"));
    }

    /// A client waits for `restored` before deciding a saved tab is gone, so it arrives even
    /// when the file holding what was lost cannot be kept aside, and says nothing is saved.
    #[test]
    fn restored_arrives_when_what_was_lost_cannot_be_kept_aside() {
        let dir = scratch("unkept");
        // No file there to copy.
        let (shared, mut events) = session(&dir.join("daemon.state.json"));
        let lost = Lost { tabs: vec!["t1".to_string()], panes: vec!["p1".to_string()] };
        finish_restore(&shared, lost, None);
        let expected = proto::Restored {
            lost_tabs: vec!["t1".to_string()],
            lost_panes: vec!["p1".to_string()],
            saving_stopped: true,
        };
        assert_eq!(next_event(&mut events), Payload::Restored(expected));
        assert!(!shared.lock().restoring);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restored_arrives_when_restoring_failed_partway() {
        let dir = scratch("failed");
        let (shared, mut events) = session(&dir.join("daemon.state.json"));
        finish_restore(&shared, Lost::default(), Some("a bug"));
        let expected = proto::Restored { saving_stopped: true, ..proto::Restored::default() };
        assert_eq!(next_event(&mut events), Payload::Restored(expected));
        assert!(!shared.lock().restoring);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A directory on a hung mount never answers whether it is there, and restoring must not
    /// wait on it: the pane starts at home instead. Several of them cost one wait, not one each.
    #[test]
    fn directories_that_do_not_answer_are_given_up_on_together() {
        let hung = |_: &Path| {
            std::thread::sleep(std::time::Duration::from_secs(30));
            true
        };
        let asked = std::time::Instant::now();
        let patience = std::time::Duration::from_millis(500);
        let paths = (0..5).map(|n| PathBuf::from(format!("/hung/{n}")));
        let found = probe_directories(paths, patience, hung);
        assert_eq!(found.len(), 5);
        assert!(found.values().all(Option::is_none));
        assert!(asked.elapsed() < patience * 2, "{:?} for five", asked.elapsed());

        let gone = PathBuf::from("/nonexistent/directory");
        let found = probe_directories(
            [PathBuf::from("/"), gone.clone(), PathBuf::from("/")],
            DIRECTORY_PATIENCE,
            Path::is_dir,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[Path::new("/")], Some(true));
        assert_eq!(found[&gone], Some(false));
    }

    fn replace_request() -> Service {
        Service::Session(proto::SessionRequest {
            request: Some(session_request::Request::Replace(session_request::Replace {
                program: Some("/bin/false".to_string()),
                data: None,
            })),
        })
    }

    fn outcome(handled: Handled) -> Outcome {
        match handled {
            Handled::Reply(reply) | Handled::Snapshot(reply) => reply.outcome,
            _ => Outcome::Done,
        }
    }

    /// Until every saved tab is back, the session holds less than it will, and a handoff would
    /// hand over less. And one handoff at a time: two asked at once both get as far as the
    /// program's launch, and the second is refused when it would mark the session.
    #[test]
    fn a_daemon_restoring_or_already_being_replaced_is_not_replaced() {
        let dir = scratch("replace");
        let (shared, events) = session(&dir.join("daemon.state.json"));
        let asker = Outbox::open(&events).unwrap();
        assert_eq!(outcome(shared.lock().handle(replace_request(), &asker)), Outcome::Refused);
        shared.lock().restoring = false;
        assert!(matches!(shared.lock().handle(replace_request(), &asker), Handled::Replace(_)));
        assert!(matches!(shared.lock().handle(replace_request(), &asker), Handled::Replace(_)));
        assert_eq!(shared.lock().begin_replacing(), Ok(()));
        assert!(shared.lock().begin_replacing().is_err(), "the second is refused once marked");
        assert_eq!(outcome(shared.lock().handle(replace_request(), &asker)), Outcome::Refused);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What is handed over is what the session held when the handoff began, so while it runs a
    /// request that would change that is refused, and one that only reads is served.
    #[test]
    fn a_daemon_being_replaced_serves_only_what_changes_nothing() {
        let dir = scratch("replacing");
        let (shared, events) = session(&dir.join("daemon.state.json"));
        let asker = Outbox::open(&events).unwrap();
        shared.lock().restoring = false;
        assert!(matches!(shared.lock().handle(replace_request(), &asker), Handled::Replace(_)));
        assert_eq!(shared.lock().begin_replacing(), Ok(()));
        let close = Service::Tab(proto::TabRequest {
            request: Some(tab_request::Request::Close(tab_request::Close {
                tab: "t1".to_string(),
            })),
        });
        let snapshot = Service::Session(proto::SessionRequest {
            request: Some(session_request::Request::Snapshot(session_request::Snapshot {})),
        });
        assert_eq!(outcome(shared.lock().handle(close.clone(), &asker)), Outcome::Refused);
        assert_eq!(outcome(shared.lock().handle(snapshot, &asker)), Outcome::Done);
        shared.lock().not_replaced();
        assert_eq!(outcome(shared.lock().handle(close, &asker)), Outcome::NotThere);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The persist thread's copy and the stop's hand-over happen in holds of one lock, so a
    /// copy after the stop's must see that it has begun, never the closed session.
    #[test]
    fn a_stopping_session_hands_over_what_it_held_and_offers_nothing_after() {
        let dir = scratch("stopping");
        let (shared, _events) = session(&dir.join("daemon.state.json"));
        assert!(shared.lock().persisted_unless_stopping().is_some());
        shared.lock().close_everything();
        assert_eq!(shared.lock().persisted_unless_stopping(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
