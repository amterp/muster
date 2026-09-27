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

use std::collections::HashSet;
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
use proto::event::Event as Payload;
use proto::request::Service;
use proto::{Outcome, pane_request, session_request, tab_request};

use crate::control::Outbox;
use crate::data::Data;
use crate::detect::Detecting;
use crate::effects::{self, Report, Reported, Reports};
use crate::facts;
use crate::pane::{Ended, Pane, PaneIo, Watching};
use crate::pty::{self, Grid, Launch};
use crate::screen::{self, Appearance, Screen, Settled};
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
    /// What the daemon gives its shells: the terminfo entry and the shell integration.
    pub(crate) data: Data,
}

/// What every thread of the daemon shares.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) session: Mutex<Session>,
    /// Told when the daemon should exit: a `stop` answered, or a signal.
    pub(crate) stopping: Sender<()>,
    pub(crate) instance: u64,
}

impl Shared {
    pub(crate) fn new(
        instance: u64,
        stopping: Sender<()>,
        inherited: Vec<(OsString, OsString)>,
        places: Places,
    ) -> Arc<Shared> {
        let Places { home, overrides, reachable, data } = places;
        Arc::new_cyclic(|shared: &Weak<Shared>| {
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
            let shared = shared.clone();
            let ended: Ended = Arc::new(move |serial, status| {
                if let Some(shared) = shared.upgrade() {
                    shared.lock().ended(serial, status);
                }
            });
            let settings = proto::Settings {
                shell: Some(proto::Shell::default()),
                ..proto::Settings::default()
            };
            let settled = Arc::new(Settled {
                generation: 0,
                appearance: Appearance::of(&settings),
                scrollback: screen::DEFAULT_SCROLLBACK,
            });
            Shared {
                session: Mutex::new(Session {
                    instance,
                    seq: 0,
                    tabs: Vec::new(),
                    panes: Vec::new(),
                    settings,
                    settled,
                    deferred: Vec::new(),
                    detecting: Detecting::start(overrides),
                    app_manifests: Vec::new(),
                    manifest_loads: 0,
                    manifests_adopted: 0,
                    subscribers: Vec::new(),
                    inherited,
                    home,
                    data,
                    reachable,
                    next_serial: 0,
                    ended,
                    reports,
                    host: effects::host_name(),
                    reserved: HashSet::new(),
                    stopping: false,
                }),
                stopping,
                instance,
            }
        })
    }

    pub(crate) fn lock(&self) -> Locked<'_> {
        Locked { session: Some(poison::lock(&self.session, "daemon.session")) }
    }
}

/// The session, locked. Letting go of it does whatever work on panes' terminals the holder
/// left deferred, once the lock is released.
pub(crate) struct Locked<'a> {
    session: Option<MutexGuard<'a, Session>>,
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
        drop(session);
        for work in deferred {
            work.run();
        }
    }
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
    /// The manifests every pane's agent is detected by, which each pane's reader reads.
    detecting: Arc<Detecting>,
    /// The manifests the app last sent, by name, which every reload layers in.
    app_manifests: Vec<(String, String)>,
    /// How many `send_manifests` have been asked for, and which of them last put its manifests
    /// in use.
    manifest_loads: u64,
    manifests_adopted: u64,
    subscribers: Vec<Outbox>,
    /// The daemon's own environment, which every pane's starts from.
    inherited: Vec<(OsString, OsString)>,
    /// Where a pane starts when nothing says where.
    home: PathBuf,
    /// What the daemon gives its shells: the terminfo entry and the shell integration.
    data: Data,
    /// How a pane's programs reach this daemon, which every pane's environment says.
    reachable: spawn::Reachable,
    next_serial: u64,
    ended: Ended,
    /// Where panes send what their programs asked for, for the publisher to apply here.
    reports: Reports,
    /// This machine's name, which OSC 7 URLs from a shell here carry.
    host: String,
    /// Pane and tab names a create has claimed while its process starts, so a second create
    /// cannot claim them too.
    reserved: HashSet<String>,
    /// Set once the daemon has begun to stop, after which no pane starts.
    stopping: bool,
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
}

impl Tab {
    fn record(&self) -> proto::Tab {
        proto::Tab {
            tab: self.name.clone(),
            label: Some(self.label.clone()),
            root: Some(node_record(&self.root)),
            zoomed: self.zoomed.clone(),
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
    fn done() -> Reply {
        Reply { outcome: Outcome::Done, reason: String::new(), detail: None }
    }

    fn already() -> Reply {
        Reply { outcome: Outcome::AlreadySo, reason: String::new(), detail: None }
    }

    fn not_there(what: impl Into<String>) -> Reply {
        Reply { outcome: Outcome::NotThere, reason: what.into(), detail: None }
    }

    fn refused(why: impl Into<String>) -> Reply {
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
    pub(crate) fn load(&self) -> Manifests {
        self.detecting.load(&self.app)
    }
}

/// A `pane.read` whose pane has been found.
#[derive(Debug)]
pub(crate) struct Reading {
    io: Arc<PaneIo>,
    first_row: u64,
    rows: u32,
}

impl Reading {
    pub(crate) fn read(&self) -> Reply {
        let text = screen::page(self.first_row, self.rows, screen::PAGE_BYTES, |first, count| {
            self.io.screen().rows(first, count)
        });
        Reply { detail: Some(Box::new(Detail::Text(text))), ..Reply::done() }
    }
}

/// A create that has been checked, with its names reserved, and whose process has yet to start.
#[derive(Debug)]
pub(crate) struct Starting {
    pane: String,
    label: Option<String>,
    command: Option<String>,
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
    Beside { pane: String, side: tree::Side, ratio: f32 },
    NewTab { name: String, label: proto::Label },
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
        let reply = match service {
            Service::Session(proto::SessionRequest { request: Some(request) }) => match request {
                S::Snapshot(_) => Reply {
                    detail: Some(Box::new(Detail::Snapshot(self.snapshot()))),
                    ..Reply::done()
                },
                S::Subscribe(_) => self.subscribe(asker),
                S::SetShell(set) => self.set_shell(set),
                S::SetScrollback(set) => self.set_scrollback(set),
                S::SetPalette(set) => self.set_palette(set),
                S::SendManifests(manifests) => return self.send_manifests(manifests),
                S::SetClipboardWrite(set) => self.set_clipboard_write(set),
                S::SetCursor(set) => self.set_cursor(set),
                S::Stop(_) => {
                    self.close_everything();
                    Reply::done()
                }
            },
            Service::Tab(proto::TabRequest { request: Some(request) }) => match request {
                T::Close(close) => self.close_tab(&close.tab),
                T::Rename(rename) => self.rename_tab(rename),
                T::SetSplitRatio(set) => self.set_split_ratio(&set),
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
                P::Read(read) => match self.pane_index(&read.pane) {
                    None => Reply::not_there(format!("no pane {} on this daemon", read.pane)),
                    Some(index) => {
                        return Handled::Read(Box::new(Reading {
                            io: Arc::clone(&self.panes[index].io),
                            first_row: read.first_row,
                            rows: read.rows,
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
    }

    /// Closes every tab, and so every pane, in the order they were opened, and starts no more.
    pub(crate) fn close_everything(&mut self) {
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
        }
    }

    fn subscribe(&mut self, asker: &Outbox) -> Reply {
        if !self.subscribers.iter().any(|subscriber| subscriber.id == asker.id) {
            self.subscribers.push(asker.clone());
        }
        Reply { detail: Some(Box::new(Detail::Snapshot(self.snapshot()))), ..Reply::done() }
    }

    /// Numbers an event and queues it for every subscriber. One that cannot take it has fallen
    /// too far behind to catch up from here, so it is dropped and resubscribes.
    fn emit(&mut self, payload: Payload) {
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
        let target = self.target(create.placement, &create.pane)?;
        let neighbour = match &target {
            Target::Beside { pane, .. } => self.pane_index(pane).map(|index| &self.panes[index]),
            Target::NewTab { .. } => None,
        };
        let grid = match create.grid.map(grid) {
            Some(grid) => grid.map_err(Reply::refused)?,
            None => neighbour.map_or(Grid::FALLBACK, |pane| pane.io.grid()),
        };
        let cwd = match create.cwd.filter(|cwd| !cwd.is_empty()) {
            Some(cwd) => PathBuf::from(cwd),
            None => neighbour.and_then(Pane::live_cwd).unwrap_or_else(|| self.home.clone()),
        };

        let shell = self.settings.shell.clone().unwrap_or_default();
        let login = shell.mode() != proto::ShellMode::NonLogin;
        let environment = spawn::environment(
            &self.inherited,
            &create.env,
            &create.pane,
            create.command.as_deref(),
            &self.data.terminfo(),
            &self.reachable,
            self.settings.cursor.as_ref(),
        );
        let (argv, environment) = spawn::start(
            &pty::shell(shell.command.as_deref(), &self.inherited),
            login,
            create.command.is_some(),
            environment,
            &self.data.shell_integration(),
        );

        self.reserved.insert(create.pane.clone());
        if let Target::NewTab { name, .. } = &target {
            self.reserved.insert(name.clone());
        }
        Ok(Starting {
            pane: create.pane,
            label: create.label,
            command: create.command,
            target,
            grid,
            cwd,
            argv,
            environment,
        })
    }

    /// Finishes a create once its process has started, or failed to. The names it reserved are
    /// released either way, and a pane whose place went away while it started is ended again.
    pub(crate) fn started(
        &mut self,
        starting: Starting,
        started: std::io::Result<(OwnedFd, Child)>,
    ) -> Reply {
        self.reserved.remove(&starting.pane);
        if let Target::NewTab { name, .. } = &starting.target {
            self.reserved.remove(name);
        }
        let program = &starting.argv[0];
        let (master, child) = match started {
            Ok(started) => started,
            Err(error) => {
                return Self::could_not_start(&starting.pane, program, &starting.cwd, &error);
            }
        };
        let gone = match &starting.target {
            _ if self.stopping => Some(Reply::refused("the daemon stopped while the pane started")),
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
            cwd: starting.cwd.display().to_string(),
            command: starting.command,
            ..proto::Pane::default()
        };
        let screen = match Screen::new(starting.grid, &self.settled) {
            Ok(screen) => screen,
            Err(error) => {
                pty::abandon(child.id().cast_signed());
                let error = std::io::Error::other(error.to_string());
                return Self::could_not_start(&starting.pane, program, &starting.cwd, &error);
            }
        };
        self.next_serial += 1;
        let serial = self.next_serial;
        let watching = Watching {
            ended: &self.ended,
            reports: &self.reports,
            host: &self.host,
            detecting: &self.detecting,
        };
        let started =
            Pane::start(record, serial, master, screen, starting.grid, Some(child), &watching);
        let pane = match started {
            Ok(pane) => pane,
            Err(error) => {
                return Self::could_not_start(&starting.pane, program, &starting.cwd, &error);
            }
        };
        log::info("daemon.pane.started", fields! { "pane" => starting.pane, "serial" => serial });
        self.emit(Payload::PaneOpened(proto::PaneOpened { pane: Some(pane.record.clone()) }));
        self.panes.push(pane);
        self.place(&starting.pane, starting.target);
        Reply::done()
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
    /// placement may not name as its own neighbour.
    fn target(&self, placement: Option<proto::Placement>, moving: &str) -> Result<Target, Reply> {
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
                valid_name("tab", &new_tab.tab).map_err(Reply::refused)?;
                if self.tab_index(&new_tab.tab).is_some() || self.reserved.contains(&new_tab.tab) {
                    return Err(Reply::refused(format!(
                        "a tab {} is already on this daemon; place the pane beside one of its panes",
                        new_tab.tab
                    )));
                }
                Ok(Target::NewTab { name: new_tab.tab, label: new_tab.label.unwrap_or_default() })
            }
        }
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
            Target::NewTab { name, label } => {
                let tab = Tab { name, label, root: Node::Pane(pane.to_string()), zoomed: None };
                let record = tab.record();
                self.tabs.push(tab);
                self.emit(Payload::TabOpened(proto::TabOpened { tab: Some(record) }));
            }
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

    /// Facts the agent in a pane states about itself.
    fn report(&mut self, report: pane_request::Report) -> Reply {
        let Some(index) = self.pane_index(&report.pane) else {
            return Reply::not_there(format!("no pane {} on this daemon", report.pane));
        };
        let record = &mut self.panes[index].record;
        let facts = match facts::apply(record.facts.as_ref(), report) {
            Ok(facts) => facts,
            Err(why) => return Reply::refused(why),
        };
        if record.facts == facts {
            return Reply::already();
        }
        record.facts = facts;
        let record = record.clone();
        self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
        Reply::done()
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
        let record = &mut self.panes[index].record;
        match report.what {
            Reported::Title(title) => {
                if record.title != title {
                    record.title = title;
                    let record = record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
            }
            Reported::Cwd(directory) => {
                let cwd = directory.display().to_string();
                if record.cwd != cwd {
                    record.cwd = cwd;
                    let record = record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
            }
            Reported::Shown(effect) => {
                self.emit(Payload::PaneEffect(proto::PaneEffect {
                    pane: name,
                    effect: Some(effect),
                }));
            }
            Reported::PasteHeld(text) => {
                self.emit(Payload::PasteHeld(proto::PasteHeld { pane: name, text }));
            }
            Reported::Agent { agent, state } => {
                if record.agent != agent || record.agent_state() != state {
                    // What an agent said about itself leaves with it. Not when a pane with no
                    // agent is first recognised: its statusline can report before the first
                    // probe lands, and that report is the new agent's.
                    if record.agent.is_some() && record.agent != agent {
                        record.facts = None;
                    }
                    record.agent = agent;
                    record.set_agent_state(state);
                    let record = record.clone();
                    self.emit(Payload::PaneChanged(proto::PaneChanged { pane: Some(record) }));
                }
            }
        }
    }

    /// A pane's process ended. Nothing to do if the pane was already closed.
    fn ended(&mut self, serial: u64, status: Option<i32>) {
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
        let target = match self.target(moved.placement, &moved.pane) {
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
        let record = &mut self.panes[index].record;
        if record.label == rename.label {
            return Reply::already();
        }
        record.label = rename.label;
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

    /// The history each pane keeps, in bytes.
    fn scrollback(&self) -> usize {
        self.settings.scrollback_bytes.map_or(screen::DEFAULT_SCROLLBACK, |bytes| {
            usize::try_from(bytes).unwrap_or(usize::MAX)
        })
    }

    /// Numbers what the settings now mean for a pane's terminal, and has every pane apply it
    /// once the lock is let go.
    fn resettle(&mut self) {
        self.settled = Arc::new(Settled {
            generation: self.settled.generation + 1,
            appearance: Appearance::of(&self.settings),
            scrollback: self.scrollback(),
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
    fn send_manifests(&mut self, sent: proto::SendManifests) -> Handled {
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
    /// each working agent idle through a startup grace, then working again.
    pub(crate) fn manifests_loaded(&mut self, loading: Loading, loaded: Manifests) -> Reply {
        if loading.load < self.manifests_adopted {
            // A later send already put its manifests in use, and they supersede these.
            return Reply::done();
        }
        self.manifests_adopted = loading.load;
        let changed = self.detecting.adopt(loaded);
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

    // -----------------------------------------------------------------------------------------
    // Lookups

    /// What a stream or input connection needs of a pane, found by name.
    pub(crate) fn pane_io(&self, pane: &str) -> Option<Arc<PaneIo>> {
        self.pane_index(pane).map(|index| Arc::clone(&self.panes[index].io))
    }

    fn pane_index(&self, pane: &str) -> Option<usize> {
        self.panes.iter().position(|candidate| candidate.record.pane == pane)
    }

    fn tab_index(&self, tab: &str) -> Option<usize> {
        self.tabs.iter().position(|candidate| candidate.name == tab)
    }

    fn tab_of(&self, pane: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.root.contains(pane))
    }
}

/// Muster's names are minted as short ASCII words (`p1w3r07bsd`). The daemon holds any name to
/// that shape, since it becomes an environment variable's value and, later, a line in a file.
fn valid_name(what: &str, name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(format!(
            "{name:?} is not a {what} name: one to 64 printable ASCII characters, no spaces"
        ));
    }
    Ok(())
}

fn valid_ratio(ratio: f32) -> Result<f32, String> {
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
