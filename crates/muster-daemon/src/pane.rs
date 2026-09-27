//! One pane: what the daemon publishes about it, the PTY it runs on, and its terminal.
//!
//! [`Pane::start`] is the only way a pane comes to exist, and it takes a PTY master, a terminal
//! and its process rather than spawning anything. A pane this daemon started has its child. A
//! pane handed over by a daemon being replaced (MIP-3, section 10) arrives as a master, its
//! record and a terminal rebuilt from a replay, with a process that is some other daemon's
//! child - so nothing about a pane depends on this process being the PTY's parent.
//!
//! Each pane runs three threads: a reader that feeds every chunk of output to the pane's
//! terminal, a writer that is the only thing writing to the program, and, with a child, a
//! waiter that reaps it. None of them takes the session lock except the waiter, whose pane
//! has ended.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;

use crate::detect::{self, Detecting, Detection};
use crate::effects::{self, Happened, Reported, Reports};
use crate::hold::{Hold, Leaving};
use crate::persist::Persister;
use crate::process;
use crate::pty;
use crate::pty::Grid;
use crate::screen::{Screen, Settled};
use crate::stream::{self, Bridge, Refusal};
use crate::writer::{self, Encoding, Input, Writer};

/// Told when a pane's process has ended, with the pane's serial and, when this daemon saw the
/// process end, how it ended.
pub(crate) type Ended = Arc<dyn Fn(u64, Option<i32>) + Send + Sync>;

/// How long after output the reader looks at which directory the pane's program is in, for a
/// shell that does not report it with OSC 7. Checked at most this often during a flood.
const CWD_CADENCE: Duration = Duration::from_millis(100);

/// How long what runs in a closed pane has to end after its hang-up before it is killed. A group
/// id reused within it would be killed too, which the kernel makes unlikely: it hands out ids in
/// order, and a group lives while any of its processes do.
pub(crate) const KILL_GRACE: Duration = Duration::from_secs(3);

/// How many hung-up panes may still have processes to kill, for a stopping daemon to wait on:
/// once it exits, nothing would kill them.
struct Killing {
    outstanding: Mutex<usize>,
    done: Condvar,
}

static KILLING: Killing = Killing { outstanding: Mutex::new(0), done: Condvar::new() };

/// Waits until every process group hung up so far is gone or killed, at most `within`.
pub(crate) fn wait_for_kills(within: Duration) {
    let outstanding = poison::lock(&KILLING.outstanding, "daemon.pane.killing");
    let _ = KILLING.done.wait_timeout_while(outstanding, within, |outstanding| *outstanding > 0);
}

/// Kills whatever of `groups` is still running [`KILL_GRACE`] after its hang-up: a program that
/// ignores SIGHUP would otherwise outlive its pane for as long as the machine runs.
fn kill_after_grace(mut groups: Vec<i32>, pane: &str) {
    if groups.is_empty() {
        return;
    }
    let finished = || {
        *poison::lock(&KILLING.outstanding, "daemon.pane.killing") -= 1;
        KILLING.done.notify_all();
    };
    *poison::lock(&KILLING.outstanding, "daemon.pane.killing") += 1;
    let name = format!("kill {pane}");
    let killing = pane.to_string();
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        let pane = killing;
        let deadline = Instant::now() + KILL_GRACE;
        loop {
            groups.retain(|&group| pty::group_exists(group));
            if groups.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for group in groups {
            pty::kill_group(group);
            log::warn(
                "daemon.pane.killed",
                fields! {
                    "pane" => pane,
                    "group" => group,
                    "impact" => "processes of a closed pane that were still running after its \
                                 hang-up were killed",
                    "check" => "whether the pane's program traps or ignores SIGHUP",
                },
            );
        }
        finished();
    });
    if let Err(error) = spawned {
        finished();
        log::error(
            "daemon.pane.not_killed",
            fields! {
                "pane" => pane,
                "error" => error,
                "impact" => "a program in the closed pane that ignores its hang-up keeps running",
                "check" => "whether the daemon is out of threads",
            },
        );
    }
}

/// What a pane's threads and the daemon's connections share.
#[derive(Debug)]
pub(crate) struct PaneIo {
    pub(crate) serial: u64,
    master: Arc<OwnedFd>,
    /// The pane's lock: its terminal, and where its output stands.
    screen: Mutex<Screen>,
    /// The pane's size ([`Grid::to_bits`]), written under the pane's lock and read without it,
    /// so that nothing holding the session lock waits on a pane's.
    grid: AtomicU64,
    /// The pane's modes as the writer encodes against them, refreshed under the pane's lock.
    encoding: Arc<Mutex<Encoding>>,
    input: SyncSender<Input>,
    /// Set when the session forgets the pane, under the session's lock, before anything else of
    /// the pane is let go: after it, the pane's name may belong to another pane.
    closed: AtomicBool,
    /// Set when the manifests this pane's agent is detected by have changed, for its reader to
    /// start detection over on its next tick.
    reset_detection: AtomicBool,
    /// Wakes the reader waiting for its bridge's credit.
    flow: Flow,
    /// Told when the pane's size changes, which a restart keeps. None in tests of a lone pane.
    persister: Option<Arc<Persister>>,
    /// Stops the reader while the pane is handed to another daemon.
    hold: Hold,
    /// The size a bridge asked for while the pane was held, applied only if the handoff fails.
    deferred_resize: Mutex<Option<Grid>>,
    /// Where the reader's agent detection stood when it was last held.
    carried: Mutex<Option<proto::handoff::Detection>>,
}

/// A count of changes to a pane's bridge - attached, detached, credited, closed - that a reader
/// waiting for credit sleeps on. Its own lock, apart from the pane's, so the reader waits holding
/// neither.
#[derive(Debug, Default)]
struct Flow {
    changes: Mutex<u64>,
    changed: Condvar,
}

impl Flow {
    fn seen(&self) -> u64 {
        *poison::lock(&self.changes, "daemon.pane.flow")
    }

    fn change(&self) {
        *poison::lock(&self.changes, "daemon.pane.flow") += 1;
        self.changed.notify_all();
    }

    /// Sleeps until something changes after `seen`, or `deadline`. False at the deadline.
    fn wait(&self, seen: u64, deadline: Instant) -> bool {
        let mut changes = poison::lock(&self.changes, "daemon.pane.flow");
        while *changes == seen {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            changes = self
                .changed
                .wait_timeout(changes, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl PaneIo {
    pub(crate) fn screen(&self) -> MutexGuard<'_, Screen> {
        poison::lock(&self.screen, "daemon.pane.screen")
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn reset_detection(&self) {
        self.reset_detection.store(true, Ordering::Release);
    }

    pub(crate) fn take_detection_reset(&self) -> bool {
        self.reset_detection.swap(false, Ordering::AcqRel)
    }

    /// The process group holding the pane's terminal, when it says.
    pub(crate) fn foreground_group(&self) -> Option<i32> {
        pty::foreground_group(self.master.as_fd())
    }

    pub(crate) fn grid(&self) -> Grid {
        Grid::from_bits(self.grid.load(Ordering::Acquire))
    }

    /// Applies what the app last said to the pane's terminal, unless it has something newer.
    pub(crate) fn settle(&self, settled: &Settled) {
        let Some(settling) = self.screen().settle(settled) else { return };
        if let Some(report) = settling.report {
            self.queue(Input::Reply(report.to_vec()));
        }
        if let Err(error) = settling.scrollback {
            log::warn(
                "daemon.pane.scrollback_unchanged",
                fields! {
                    "serial" => self.serial,
                    "error" => error,
                    "impact" => "this pane keeps the history limit it had; new panes get the \
                                 new one",
                },
            );
        }
    }

    /// Queues something for the program to read. False when the queue is full or the pane has
    /// gone; the caller decides whether that is worth saying.
    pub(crate) fn queue(&self, input: Input) -> bool {
        match self.input.try_send(input) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// Waits for the pane's bridge to have room for more output, for at most `grace`, holding no
    /// lock while it waits. Returns at once with no bridge attached, or one already behind; past
    /// the grace the output goes on, and the bridge falls behind.
    ///
    /// This is what lets a burst reach a surface that keeps up whole, as Ghostty's own reader
    /// waits for its parser: the program is slowed to the bridge's pace instead of the bridge
    /// missing output. A bridge that has stopped reading holds the program only for the grace.
    fn wait_for_room(&self, grace: Duration) {
        let deadline = Instant::now() + grace;
        loop {
            // Read before looking, so a change between the look and the sleep ends the sleep.
            let seen = self.flow.seen();
            if !self.screen().bridge_is_full() || !self.flow.wait(seen, deadline) {
                return;
            }
        }
    }

    /// Output for the pane, from its program or on its behalf: to its bridge and its terminal,
    /// then the modes it left read out for the writer.
    fn output(&self, bytes: &[u8]) -> Vec<Happened> {
        let mut screen = self.screen();
        let happened = screen.output(bytes);
        poison::lock(&self.encoding, "daemon.pane.encoding").refresh(screen.terminal());
        happened
    }

    /// Resets the pane's terminal and its surface's, as Ghostty's `reset` does. The program is
    /// not told.
    pub(crate) fn reset(&self) {
        self.output(b"\x1bc");
    }

    /// Attaches a bridge to the pane, at `grid` when it says one.
    pub(crate) fn attach(
        &self,
        bridge: Bridge,
        grid: Option<Grid>,
        takeover: bool,
    ) -> Result<(), Refusal> {
        let mut screen = self.screen();
        // Checked under the pane's lock, which the hang-up takes after setting the flag: a
        // bridge either sees the pane closed here, or is registered in time to be detached.
        if self.is_closed() {
            return Err(bridge.refused("the pane has closed".to_string()));
        }
        if let Some(grid) = grid.filter(|&grid| grid != self.grid()) {
            self.resize_locked(&mut screen, grid);
        }
        let attached = screen.attach(bridge, takeover);
        drop(screen);
        self.flow.change();
        attached
    }

    pub(crate) fn detach(&self, bridge: u64) {
        self.screen().detach(bridge);
        self.flow.change();
    }

    pub(crate) fn acknowledge(&self, bridge: u64, bytes: u64) {
        self.screen().acknowledge(bridge, bytes);
        self.flow.change();
    }

    /// Tells the pane's bridge why the pane is going, and lets go of it.
    pub(crate) fn close(&self, reason: proto::DetachReason) {
        self.screen().close(reason);
        self.flow.change();
    }

    /// Stops the pane's reader once its terminal has everything the reader took from the PTY,
    /// so what the program writes next waits in the PTY for whoever reads it next. False when
    /// the reader did not stop `within`.
    pub(crate) fn hold_reader(&self, within: Duration) -> bool {
        self.hold.hold(within)
    }

    /// Lets the reader go on, and applies a resize asked for while it was held.
    pub(crate) fn release_reader(&self) {
        self.hold.release();
        let deferred = poison::lock(&self.deferred_resize, "daemon.pane.deferred_resize").take();
        if let Some(grid) = deferred {
            self.resize(grid);
        }
    }

    /// What a daemon taking the pane over rebuilds its terminal from: a replay of it, at its
    /// size. Composed under the pane's lock, so the two agree.
    pub(crate) fn replay(&self) -> (Vec<u8>, Grid) {
        let screen = self.screen();
        (screen.terminal().replay(), self.grid())
    }

    fn carry(&self, detection: proto::handoff::Detection) {
        *poison::lock(&self.carried, "daemon.pane.carried") = Some(detection);
    }

    /// Where the pane's agent detection stood when its reader was held, for the daemon it is
    /// handed to.
    pub(crate) fn carried_detection(&self) -> Option<proto::handoff::Detection> {
        poison::lock(&self.carried, "daemon.pane.carried").take()
    }

    pub(crate) fn master(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }

    /// The pane's program and its terminal, both at a new size. A pane keeps its last size
    /// when its bridge goes.
    ///
    /// Not while the pane is held for a handoff: its replay may already be composed at the old
    /// size, and the new daemon's terminal would disagree with the PTY both daemons share. The
    /// size waits for the handoff to fail; if it succeeds, the bridge is detached and attaches
    /// again with its size.
    pub(crate) fn resize(&self, grid: Grid) {
        let mut screen = self.screen();
        if self.hold.is_held() {
            *poison::lock(&self.deferred_resize, "daemon.pane.deferred_resize") = Some(grid);
            return;
        }
        if grid != self.grid() {
            self.resize_locked(&mut screen, grid);
        }
    }

    fn resize_locked(&self, screen: &mut Screen, grid: Grid) {
        poison::lock(&self.encoding, "daemon.pane.encoding").resize(grid);
        let resized = pty::set_size(self.master.as_fd(), grid)
            .map_err(|error| error.to_string())
            .and_then(|()| screen.resize(grid).map_err(|error| error.to_string()));
        match resized {
            Ok(()) => {
                self.grid.store(grid.to_bits(), Ordering::Release);
                // A size is no event, so nothing else would tell the persister. Its lock is
                // taken last by everything that takes it, so taking it under the pane's is safe.
                if let Some(persister) = &self.persister {
                    persister.changed();
                }
            }
            Err(error) => {
                log::warn(
                    "daemon.pane.not_resized",
                    fields! {
                        "serial" => self.serial,
                        "error" => error,
                        "impact" => "the pane's program and its surface may disagree about its size \
                                     until the next resize",
                    },
                );
            }
        }
    }

    /// Sends what a write to the pane's terminal asked for where it goes: answers to the
    /// program, everything else to the session.
    fn dispatch(&self, happened: Vec<Happened>, heard: &mut Heard, reports: &Reports) {
        for happening in happened {
            match happening {
                // Dropped when the queue is full rather than waiting for the program to read:
                // the reader must never block on the program it is reading.
                Happened::Reply(bytes) => {
                    self.queue(Input::Reply(bytes));
                }
                // Every title write is sent, so one the queue drops is replaced by the next.
                Happened::Title(title) => {
                    reports.send(self.serial, Reported::Title(title));
                }
                Happened::Pwd(url) => {
                    if let Some(directory) = effects::local_directory(&url, &heard.host) {
                        heard.reports_directory = true;
                        heard.moved_to(directory, self.serial, reports);
                    }
                }
                Happened::Shown(shown) => {
                    reports.send(self.serial, Reported::Shown(shown));
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Pane {
    /// What the protocol says about this pane. The fields a restart needs are persisted from
    /// here (`persist.rs`); the rest are observations.
    pub(crate) record: proto::Pane,
    /// Tells this pane's process apart from a later pane given the same name.
    pub(crate) serial: u64,
    pub(crate) io: Arc<PaneIo>,
    /// The write end of a pipe the reader and writer also poll. Dropping it ends them both,
    /// which is how a pane lets go of its master without closing a descriptor another thread
    /// is using.
    wake: OwnedFd,
    /// The pane's process, the leader of its own session.
    process: Option<i32>,
    /// Whether that process is another daemon's child, which this one never reaps.
    adopted: bool,
}

/// A pane's process, as the daemon holding the pane knows it.
#[derive(Debug)]
pub(crate) enum Process {
    /// Started by this daemon, which waits for it and hears how it ended.
    Child(Child),
    /// Started by the daemon this one replaced, whose child it stays: the PTY closing is the
    /// only word of its end, and whoever adopts it when that daemon exits reaps it.
    Adopted(i32),
}

impl Process {
    pub(crate) fn pid(&self) -> i32 {
        match self {
            Process::Child(child) => child.id().cast_signed(),
            Process::Adopted(pid) => *pid,
        }
    }
}

/// What starts watching a pane.
pub(crate) struct Watching<'a> {
    pub(crate) ended: &'a Ended,
    pub(crate) reports: &'a Reports,
    pub(crate) host: &'a str,
    pub(crate) detecting: &'a Arc<Detecting>,
    pub(crate) persister: &'a Arc<Persister>,
    /// Whether the reader starts held, as a pane handed over is until the handoff commits.
    pub(crate) held: bool,
    /// Where a pane handed over had got to in detecting its agent.
    pub(crate) detection: Option<&'a proto::handoff::Detection>,
}

impl Pane {
    /// Starts watching `master`: a reader that feeds its output to `screen`, a writer for its
    /// input and, when the process is this daemon's child, a waiter that reaps it and says how
    /// it ended.
    ///
    /// With a child, the child ending is what ends the pane, even if a background job still
    /// holds the terminal. Otherwise the PTY closing is the only word there will be.
    pub(crate) fn start(
        record: proto::Pane,
        serial: u64,
        master: OwnedFd,
        screen: Screen,
        grid: Grid,
        process: Option<Process>,
        watching: &Watching<'_>,
    ) -> io::Result<Pane> {
        let child = matches!(process, Some(Process::Child(_)));
        let process = process.map(|process| process.pid());
        let detection = match watching.detection {
            Some(carried) => {
                Detection::resumed(process, carried, Instant::now(), screen.title_writes())
            }
            None => Detection::new(process, Instant::now()),
        };
        // Every way this can fail leaves a started process nobody will wait for, so each one
        // ends and reaps it before saying so. An adopted one is still its own daemon's.
        let failed = |error: io::Error| {
            if let Some(pid) = process.filter(|_| child) {
                pty::abandon(pid);
            }
            error
        };
        let hold = Hold::new(watching.held).map_err(failed)?;
        nonblocking(&master).map_err(failed)?;
        let (wake_read, wake) = pipe().map_err(failed)?;
        let writer_wake = writer::duplicate(&wake_read).map_err(failed)?;
        let master = Arc::new(master);
        let encoding = Encoding::new(screen.terminal(), grid)
            .map_err(|error| failed(io::Error::other(error.to_string())))?;
        let encoding = Arc::new(Mutex::new(encoding));
        let (input, queued) = writer::queue();
        let io = Arc::new(PaneIo {
            serial,
            master: Arc::clone(&master),
            screen: Mutex::new(screen),
            grid: AtomicU64::new(grid.to_bits()),
            encoding: Arc::clone(&encoding),
            input,
            closed: AtomicBool::new(false),
            reset_detection: AtomicBool::new(false),
            flow: Flow::default(),
            persister: Some(Arc::clone(watching.persister)),
            hold,
            deferred_resize: Mutex::new(None),
            carried: Mutex::new(None),
        });
        let pane = record.pane.clone();

        let writing = Arc::clone(&master);
        let writer = Writer::new(
            pane.clone(),
            serial,
            encoding,
            Arc::downgrade(&io),
            watching.reports.clone(),
        );
        std::thread::Builder::new()
            .name(format!("write {pane}"))
            .spawn(move || writer.write(&queued, &writing, &writer_wake))
            .map_err(failed)?;

        let reader = Reader {
            io: Arc::clone(&io),
            wake: wake_read,
            ended: (!child).then(|| Arc::clone(watching.ended)),
            reports: watching.reports.clone(),
            process,
            heard: Heard {
                host: watching.host.to_string(),
                directory: PathBuf::from(&record.cwd),
                reports_directory: false,
                unsent: None,
            },
            detection,
            detecting: Arc::clone(watching.detecting),
            unsent: None,
        };
        std::thread::Builder::new()
            .name(format!("read {pane}"))
            .spawn(move || reader.run())
            .map_err(failed)?;

        if let Some(pid) = process.filter(|_| child) {
            let ended = Arc::clone(watching.ended);
            std::thread::Builder::new()
                .name(format!("wait {pane}"))
                .spawn(move || {
                    let status = wait(pid, &pane);
                    ended(serial, status);
                })
                .map_err(failed)?;
        }

        Ok(Pane { record, serial, io, wake, process, adopted: !child && process.is_some() })
    }

    pub(crate) fn process(&self) -> Option<i32> {
        self.process
    }

    /// The directory the pane is working in now: its foreground job's, else its shell's.
    pub(crate) fn live_cwd(&self) -> Option<PathBuf> {
        live_cwd(self.io.master.as_fd(), self.process)
    }

    /// Ends the pane: its bridge told why, SIGHUP to its shell's process group and to whatever
    /// holds its terminal's foreground, SIGKILL to either still there after [`KILL_GRACE`], and
    /// its master closed once its reader, its writer and the connections that looked it up let
    /// go of it. Its bridge's connection lets go at once (`Bridge::detach`); an input connection
    /// holds it only weakly.
    ///
    /// An adopted pane's process is reaped by whoever adopted it, so once it has ended its pid is
    /// free for any process: its groups are signaled only while that pid is not somebody else's.
    pub(crate) fn hang_up(self, reason: proto::DetachReason) {
        self.io.close(reason);
        let foreground = self.io.foreground_group().filter(|group| Some(*group) != self.process);
        let theirs = self.adopted
            && self.process.is_some_and(|pid| {
                !adopted_group_is_ours(
                    pty::session_of(self.io.master()) == Some(pid),
                    pty::process_exists(pid),
                )
            });
        let groups: Vec<i32> = if theirs {
            log::info(
                "daemon.pane.not_signaled",
                fields! {
                    "pane" => self.record.pane,
                    "pid" => self.process.unwrap_or_default(),
                    "why" => "its shell has ended and its pid now belongs to another process",
                },
            );
            Vec::new()
        } else {
            self.process.into_iter().chain(foreground).collect()
        };
        for &group in &groups {
            pty::hang_up(group);
        }
        kill_after_grace(groups, &self.record.pane);
        drop(self.wake);
    }
}

/// Whether an adopted pane's process group, named by its shell's pid, can only be the pane's.
/// While the shell lives it leads the session the pane's terminal belongs to. Once it has ended,
/// its group keeps the id while any process of the pane is still in it, and the kernel reuses no
/// id still in use, so a process that has the pid while not leading the pane's session is a
/// stranger's. Past this check the reuse left is within the kill grace, as for any pane.
fn adopted_group_is_ours(leads_the_terminal: bool, pid_in_use: bool) -> bool {
    leads_the_terminal || !pid_in_use
}

fn live_cwd(master: BorrowedFd<'_>, process: Option<i32>) -> Option<PathBuf> {
    pty::foreground_group(master).and_then(process::cwd).or_else(|| process.and_then(process::cwd))
}

/// What the reader has heard about where the pane's program is.
#[derive(Debug)]
struct Heard {
    host: String,
    /// The last directory published.
    directory: PathBuf,
    /// Whether the program reports its directory itself (OSC 7), after which the reader stops
    /// asking the kernel.
    reports_directory: bool,
    /// A directory the queue dropped, to send again at the next check.
    unsent: Option<PathBuf>,
}

impl Heard {
    /// Publishes a directory unless it is the last one published. One the queue dropped is kept
    /// for the next check to send again, which the reader schedules ([`retry_due`]).
    fn moved_to(&mut self, directory: PathBuf, serial: u64, reports: &Reports) {
        self.unsent = None;
        if directory == self.directory {
            return;
        }
        if reports.send(serial, Reported::Cwd(directory.clone())) {
            self.directory = directory;
        } else {
            self.unsent = Some(directory);
        }
    }
}

/// When the reader next checks the directory: as scheduled, or a cadence from now when a
/// directory the queue dropped is waiting. Not only after output, which is when the check is
/// otherwise scheduled: a pane that has gone quiet would never send it, and a restart would
/// bring the pane back in the directory before.
fn retry_due(due: Option<Instant>, heard: &Heard, now: Instant) -> Option<Instant> {
    due.or_else(|| heard.unsent.is_some().then(|| now + CWD_CADENCE))
}

struct Reader {
    io: Arc<PaneIo>,
    wake: OwnedFd,
    /// Told when the PTY closes, for a pane with no child to wait for.
    ended: Option<Ended>,
    reports: Reports,
    process: Option<i32>,
    heard: Heard,
    detection: Detection,
    detecting: Arc<Detecting>,
    /// What detection published that the queue dropped, to send again at the next tick.
    unsent: Option<Reported>,
}

impl Reader {
    /// Reads the pane's output until its PTY closes or the pane lets go.
    ///
    /// Each chunk goes to the pane's terminal under the pane's lock; what it asked for is sent
    /// on once the lock is released. Nothing on this path waits for the session lock.
    fn run(mut self) {
        let io = Arc::clone(&self.io);
        let _leaving = Leaving(&io.hold);
        let mut buffer = vec![0u8; 64 * 1024];
        // When the directory is next worth checking. The poll's timeout is the cadence, so
        // neither this check nor agent detection needs a thread of its own.
        let mut due: Option<Instant> = None;
        loop {
            // Here, with every byte read so far in the terminal, is where a handoff stops it.
            // What detection knows goes with the pane if it is being handed over.
            self.io.hold.park(
                || self.io.is_closed(),
                || self.io.carry(self.detection.carried(Instant::now())),
            );
            let master = self.io.master.as_raw_fd();
            let mut watched = [
                libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.io.hold.polled(), events: libc::POLLIN, revents: 0 },
            ];
            due = retry_due(due, &self.heard, Instant::now());
            let next = due.map_or(self.detection.due(), |due| due.min(self.detection.due()));
            let left = next.saturating_duration_since(Instant::now()).as_millis();
            let timeout = i32::try_from(left).unwrap_or(i32::MAX);
            // SAFETY: `watched` is a valid array of three pollfds for the length given.
            let ready = unsafe { libc::poll(watched.as_mut_ptr(), 3, timeout) };
            if ready == -1 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            if watched[1].revents != 0 {
                return;
            }
            if watched[2].revents != 0 {
                continue;
            }
            let now = Instant::now();
            if due.is_some_and(|due| now >= due) {
                due = None;
                self.check_directory();
            }
            if now >= self.detection.due() {
                self.detect(now);
            }
            if watched[0].revents == 0 {
                continue;
            }
            // SAFETY: `buffer` is valid for writes of its length.
            let read = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read > 0 {
                let chunk = &buffer[..read.cast_unsigned()];
                self.io.wait_for_room(stream::GRACE);
                self.detection.observe(chunk);
                let happened = self.io.output(chunk);
                self.io.dispatch(happened, &mut self.heard, &self.reports);
                if !self.heard.reports_directory && due.is_none() {
                    due = Some(Instant::now() + CWD_CADENCE);
                }
                continue;
            }
            if read == -1
                && matches!(
                    io::Error::last_os_error().kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                )
            {
                continue;
            }
            // End of file, or EIO: every process holding the terminal has closed it.
            if let Some(ended) = &self.ended {
                ended(self.io.serial, None);
            }
            return;
        }
    }

    /// Ticks the pane's agent detection, and publishes what it says changed.
    fn detect(&mut self, now: Instant) {
        let published = self.detection.tick(&self.io, &self.detecting, now).map(|publication| {
            let (agent, state) = detect::recorded(&publication);
            Reported::Agent { agent, state }
        });
        publish_agent(&self.reports, self.io.serial, published, &mut self.unsent);
    }

    /// Publishes the directory the pane's program is in: the kernel's word for a shell that
    /// does not say, or again what a shell that does said last, if the queue dropped it.
    fn check_directory(&mut self) {
        let latest = if self.heard.reports_directory {
            self.heard.unsent.take()
        } else {
            live_cwd(self.io.master.as_fd(), self.process)
        };
        if let Some(directory) = latest {
            self.heard.moved_to(directory, self.io.serial, &self.reports);
        }
    }
}

/// Sends what detection just published, or else what the queue last dropped. The detector counts
/// a publication as made once it has made it, so a dropped one would otherwise stand until the
/// agent's state next changed.
fn publish_agent(
    reports: &Reports,
    serial: u64,
    published: Option<Reported>,
    unsent: &mut Option<Reported>,
) {
    let Some(agent) = published.or_else(|| unsent.take()) else { return };
    *unsent = (!reports.send(serial, agent.clone())).then_some(agent);
}

/// Waits for a pane's process to end, and says how it ended as a shell reports it in `$?`: its
/// exit code, or 128 plus the signal that ended it.
fn wait(pid: i32, pane: &str) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    let mut status = 0;
    loop {
        // SAFETY: waitpid on this daemon's own child, writing one int.
        if unsafe { libc::waitpid(pid, &raw mut status, 0) } == pid {
            let status = std::process::ExitStatus::from_raw(status);
            return status.code().or_else(|| status.signal().map(|signal| 128 + signal));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            log::error(
                "daemon.pane.wait_failed",
                fields! {
                    "pane" => pane,
                    "error" => error,
                    "impact" => "the pane is closed without an exit status, and its process may \
                                 be left unreaped",
                },
            );
            return None;
        }
    }
}

/// The master is shared by a reader that polls before reading and a writer that must never
/// block past the pane's end, so neither wants a blocking descriptor.
fn nonblocking(master: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A pipe whose two ends are close-on-exec. A pane forked in the moment before they are marked
/// still gets neither (`descriptors.rs`).
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [-1; 2];
    // SAFETY: `ends` has room for the two descriptors pipe writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe succeeded, so both are open descriptors this function now owns.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    for end in [&read, &write] {
        // SAFETY: fcntl on a descriptor this function owns.
        if unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

#[cfg(test)]
impl PaneIo {
    /// A pane with no program and no threads, for tests of what connections do with one.
    pub(crate) fn idle(serial: u64) -> Arc<PaneIo> {
        use crate::screen::{Appearance, DEFAULT_SCROLLBACK};
        let master = OwnedFd::from(std::fs::File::open("/dev/null").expect("/dev/null"));
        let settled = Settled {
            generation: 0,
            appearance: Appearance::of(&proto::Settings::default()),
            scrollback: DEFAULT_SCROLLBACK,
        };
        let grid = Grid::FALLBACK;
        let screen = Screen::new(grid, &settled).expect("a terminal");
        let encoding = Encoding::new(screen.terminal(), grid).expect("encoders");
        let (input, _) = writer::queue();
        Arc::new(PaneIo {
            serial,
            master: Arc::new(master),
            screen: Mutex::new(screen),
            grid: AtomicU64::new(grid.to_bits()),
            encoding: Arc::new(Mutex::new(encoding)),
            input,
            closed: AtomicBool::new(false),
            reset_detection: AtomicBool::new(false),
            flow: Flow::default(),
            persister: None,
            hold: Hold::new(false).expect("a pipe"),
            deferred_resize: Mutex::new(None),
            carried: Mutex::new(None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_adopted_group_is_signaled_unless_its_pid_is_now_a_strangers() {
        assert!(adopted_group_is_ours(true, true), "the shell still leads its terminal");
        assert!(adopted_group_is_ours(false, false), "the shell is gone and its pid unused");
        assert!(!adopted_group_is_ours(false, true), "another process has the shell's pid");
    }

    fn heard() -> Heard {
        Heard {
            host: String::new(),
            directory: PathBuf::from("/"),
            reports_directory: false,
            unsent: None,
        }
    }

    /// The queue to the session drops a report when it is full. A directory dropped there must
    /// still reach the pane's record, which a restart restores the pane into.
    #[test]
    fn a_directory_the_queue_dropped_is_sent_at_the_next_check() {
        let (reports, received) = Reports::with_depth(1);
        reports.send(1, Reported::Title("filler".to_string()));
        let mut heard = heard();
        heard.moved_to(PathBuf::from("/tmp"), 1, &reports);
        assert!(
            received
                .try_recv()
                .is_ok_and(|report| report.what == Reported::Title("filler".to_string()))
        );
        heard.moved_to(PathBuf::from("/tmp"), 1, &reports);
        assert_eq!(
            received.try_recv().map(|report| report.what),
            Ok(Reported::Cwd(PathBuf::from("/tmp")))
        );
    }

    #[test]
    fn a_dropped_directory_is_checked_again_even_in_a_quiet_pane() {
        let (reports, _received) = Reports::with_depth(1);
        reports.send(1, Reported::Title("filler".to_string()));
        let mut heard = Heard { reports_directory: true, ..heard() };
        let now = Instant::now();
        assert_eq!(retry_due(None, &heard, now), None, "nothing waiting, nothing scheduled");
        heard.moved_to(PathBuf::from("/tmp"), 1, &reports);
        assert_eq!(retry_due(None, &heard, now), Some(now + CWD_CADENCE));
        let sooner = now + Duration::from_millis(1);
        assert_eq!(retry_due(Some(sooner), &heard, now), Some(sooner), "a check already due");
    }

    #[test]
    fn an_agent_state_the_queue_dropped_is_sent_at_the_next_tick() {
        let (reports, received) = Reports::with_depth(1);
        reports.send(1, Reported::Title("filler".to_string()));
        let working = Reported::Agent {
            agent: Some("claude".to_string()),
            state: proto::AgentState::Working,
        };
        let mut unsent = None;
        publish_agent(&reports, 1, Some(working.clone()), &mut unsent);
        assert!(received.try_recv().is_ok(), "the filler");
        publish_agent(&reports, 1, None, &mut unsent);
        assert_eq!(received.try_recv().map(|report| report.what), Ok(working));
        publish_agent(&reports, 1, None, &mut unsent);
        assert!(received.try_recv().is_err(), "sent once it fitted, and not again");
    }

    #[test]
    fn a_closed_pane_refuses_a_bridge() {
        let io = PaneIo::idle(1);
        io.mark_closed();
        let (bridge, _frames) = Bridge::for_test();
        let refused = io.attach(bridge, None, false).expect_err("a closed pane takes no bridge");
        assert!(refused.reason.contains("closed"), "{}", refused.reason);
    }

    /// A pane whose bridge's window is full, and the bridge's id.
    fn full() -> (Arc<PaneIo>, u64, std::sync::mpsc::Receiver<Vec<u8>>) {
        let io = PaneIo::idle(1);
        let (bridge, frames) = Bridge::for_test();
        let id = bridge.id();
        io.attach(bridge, None, false).expect("attached");
        io.output(&vec![b'x'; usize::try_from(stream::WINDOW).unwrap()]);
        assert!(io.screen().bridge_is_full());
        (io, id, frames)
    }

    /// How long a reader waits on a full window, with a grace far longer than the test, when
    /// `change` happens a moment after it starts waiting.
    fn waited(io: &Arc<PaneIo>, change: impl FnOnce()) -> Duration {
        let waiting = Arc::clone(io);
        let reader = std::thread::spawn(move || {
            let started = Instant::now();
            waiting.wait_for_room(Duration::from_secs(30));
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(20));
        change();
        reader.join().expect("the reader")
    }

    const PROMPT: Duration = Duration::from_secs(5);

    #[test]
    fn a_reader_with_no_bridge_or_room_does_not_wait() {
        let io = PaneIo::idle(1);
        let started = Instant::now();
        io.wait_for_room(Duration::from_secs(30));
        assert!(started.elapsed() < PROMPT, "no bridge, nobody to wait for");
    }

    #[test]
    fn a_reader_waits_out_the_grace_when_nothing_changes() {
        let (io, _, _frames) = full();
        let started = Instant::now();
        io.wait_for_room(Duration::from_millis(50));
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn credit_ends_the_wait() {
        let (io, id, _frames) = full();
        assert!(waited(&io, || io.acknowledge(id, 1)) < PROMPT);
    }

    #[test]
    fn a_bridge_going_ends_the_wait() {
        let (io, id, _frames) = full();
        assert!(waited(&io, || io.detach(id)) < PROMPT);
    }

    #[test]
    fn a_takeover_ends_the_wait() {
        let (io, _, _frames) = full();
        let (another, _more) = Bridge::for_test();
        assert!(waited(&io, || io.attach(another, None, true).expect("taken over")) < PROMPT);
    }

    #[test]
    fn the_pane_closing_ends_the_wait() {
        let (io, _, _frames) = full();
        assert!(waited(&io, || io.close(proto::DetachReason::Closed)) < PROMPT);
    }
}
