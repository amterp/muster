//! One pane: what the daemon publishes about it, the PTY it runs on, and its terminal.
//!
//! [`Pane::start`] is the only way a pane comes to exist, and it takes a PTY master, a terminal
//! and an optional child rather than spawning anything. A pane this daemon started has its
//! child. A pane handed over by a daemon being replaced (MIP-3, section 10) will arrive as a
//! master, its record and a terminal rebuilt from a replay, with no child, because its process
//! is some other daemon's child - so nothing about a pane depends on this process being the
//! PTY's parent.
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
    fn close(&self, reason: proto::DetachReason) {
        self.screen().close(reason);
        self.flow.change();
    }

    /// The pane's program and its terminal, both at a new size. A pane keeps its last size
    /// when its bridge goes.
    pub(crate) fn resize(&self, grid: Grid) {
        let mut screen = self.screen();
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
            Ok(()) => self.grid.store(grid.to_bits(), Ordering::Release),
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
                Happened::Title(title) => reports.send(self.serial, Reported::Title(title)),
                Happened::Pwd(url) => {
                    if let Some(directory) = effects::local_directory(&url, &heard.host) {
                        heard.reports_directory = true;
                        heard.moved_to(directory, self.serial, reports);
                    }
                }
                Happened::Shown(shown) => reports.send(self.serial, Reported::Shown(shown)),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct Pane {
    /// What the protocol says about this pane. The fields a restart needs are persisted from
    /// here (a later card); the rest are observations.
    pub(crate) record: proto::Pane,
    /// Tells this pane's process apart from a later pane given the same name.
    pub(crate) serial: u64,
    pub(crate) io: Arc<PaneIo>,
    /// The write end of a pipe the reader and writer also poll. Dropping it ends them both,
    /// which is how a pane lets go of its master without closing a descriptor another thread
    /// is using.
    wake: OwnedFd,
    /// The process this daemon started, when it started one: the leader of its own session.
    process: Option<i32>,
}

/// What starts watching a pane.
pub(crate) struct Watching<'a> {
    pub(crate) ended: &'a Ended,
    pub(crate) reports: &'a Reports,
    pub(crate) host: &'a str,
    pub(crate) detecting: &'a Arc<Detecting>,
}

impl Pane {
    /// Starts watching `master`: a reader that feeds its output to `screen`, a writer for its
    /// input and, when there is a child, a waiter that reaps it and says how it ended.
    ///
    /// With a child, the child ending is what ends the pane, even if a background job still
    /// holds the terminal. Without one, the PTY closing is the only word there will be.
    pub(crate) fn start(
        record: proto::Pane,
        serial: u64,
        master: OwnedFd,
        screen: Screen,
        grid: Grid,
        child: Option<Child>,
        watching: &Watching<'_>,
    ) -> io::Result<Pane> {
        let process = child.map(|child| child.id().cast_signed());
        // Every way this can fail leaves a started process nobody will wait for, so each one
        // ends and reaps it before saying so.
        let failed = |error: io::Error| {
            if let Some(pid) = process {
                pty::abandon(pid);
            }
            error
        };
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
            ended: process.is_none().then(|| Arc::clone(watching.ended)),
            reports: watching.reports.clone(),
            process,
            heard: Heard {
                host: watching.host.to_string(),
                directory: PathBuf::from(&record.cwd),
                reports_directory: false,
            },
            detection: Detection::new(process, Instant::now()),
            detecting: Arc::clone(watching.detecting),
        };
        std::thread::Builder::new()
            .name(format!("read {pane}"))
            .spawn(move || reader.run())
            .map_err(failed)?;

        if let Some(pid) = process {
            let ended = Arc::clone(watching.ended);
            std::thread::Builder::new()
                .name(format!("wait {pane}"))
                .spawn(move || {
                    let status = wait(pid, &pane);
                    ended(serial, status);
                })
                .map_err(failed)?;
        }

        Ok(Pane { record, serial, io, wake, process })
    }

    /// The directory the pane is working in now: its foreground job's, else its shell's.
    pub(crate) fn live_cwd(&self) -> Option<PathBuf> {
        live_cwd(self.io.master.as_fd(), self.process)
    }

    /// Ends the pane: its bridge told why, SIGHUP to its shell's process group and to whatever
    /// holds its terminal's foreground, then its master closed once its reader, its writer and
    /// the connections that looked it up let go of it. Its bridge's connection lets go at once
    /// (`Bridge::detach`); an input connection holds it only weakly.
    pub(crate) fn hang_up(self, reason: proto::DetachReason) {
        self.io.close(reason);
        let foreground = self.io.foreground_group().filter(|group| Some(*group) != self.process);
        for group in self.process.into_iter().chain(foreground) {
            pty::hang_up(group);
        }
        drop(self.wake);
    }
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
}

impl Heard {
    fn moved_to(&mut self, directory: PathBuf, serial: u64, reports: &Reports) {
        if directory != self.directory {
            self.directory.clone_from(&directory);
            reports.send(serial, Reported::Cwd(directory));
        }
    }
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
}

impl Reader {
    /// Reads the pane's output until its PTY closes or the pane lets go.
    ///
    /// Each chunk goes to the pane's terminal under the pane's lock; what it asked for is sent
    /// on once the lock is released. Nothing on this path waits for the session lock.
    fn run(mut self) {
        let mut buffer = vec![0u8; 64 * 1024];
        // When the directory is next worth checking. The poll's timeout is the cadence, so
        // neither this check nor agent detection needs a thread of its own.
        let mut due: Option<Instant> = None;
        loop {
            let master = self.io.master.as_raw_fd();
            let mut watched = [
                libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            let next = due.map_or(self.detection.due(), |due| due.min(self.detection.due()));
            let left = next.saturating_duration_since(Instant::now()).as_millis();
            let timeout = i32::try_from(left).unwrap_or(i32::MAX);
            // SAFETY: `watched` is a valid array of two pollfds for the length given.
            let ready = unsafe { libc::poll(watched.as_mut_ptr(), 2, timeout) };
            if ready == -1 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            if watched[1].revents != 0 {
                return;
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
        if let Some(publication) = self.detection.tick(&self.io, &self.detecting, now) {
            let (agent, state) = detect::recorded(&publication);
            self.reports.send(self.io.serial, Reported::Agent { agent, state });
        }
    }

    /// Publishes the directory the pane's program is in, for a shell that does not say.
    fn check_directory(&mut self) {
        if self.heard.reports_directory {
            return;
        }
        if let Some(directory) = live_cwd(self.io.master.as_fd(), self.process) {
            self.heard.moved_to(directory, self.io.serial, &self.reports);
        }
    }
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
