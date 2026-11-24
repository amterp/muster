//! The daemon's own log (MIP-3 section 1): a file beside its socket that it keeps bounded
//! itself, and the same records handed to any control connection that follows them.
//!
//! Never a run's log. A daemon outlives the run that started it, so a run's file would stop
//! describing it the moment that run ended, and would grow or dangle after. An app that wants
//! the daemon's side in its run's timeline follows the log over its control connection, which
//! reaches a daemon on a devenv the same way as one on this machine.
//!
//! What a person typed is never in it: nothing in the daemon logs input's content, and
//! `MUSTER_LOG_INPUT` does not change that here.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use muster_core::diagnostics::log::{self, LogLevel, LogRecord, LogSink};
use muster_core::diagnostics::sink;
use muster_daemon_proto as proto;
use prost::Message;

use crate::control::Outbox;

/// Past this the file is rotated to `<name>.log.1`, replacing the one before: at most twice this
/// on disk.
const FILE_BYTES: u64 = 4 << 20;

/// How many records the daemon holds for a client that starts following.
const KEPT: usize = 1000;

/// The file beside a daemon's socket: `~/.muster/daemon/<install>.log`.
pub(crate) fn path_for(socket: &Path) -> PathBuf {
    socket.with_extension("log")
}

/// The last record of a log whose daemon never said it ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LastRecord {
    pub(crate) time: String,
    pub(crate) event: String,
}

#[derive(serde::Deserialize)]
struct Logged {
    time: String,
    pid: i32,
    event: String,
}

const STARTED: &str = "daemon.started";
const SERVING: &str = "daemon.handoff.serving";
const STOPPED: &str = "daemon.stopped";
const FAILED: &str = "daemon.failed";
/// The first record of a file the log was rotated into, by the daemon writing the log, so each
/// file says whose run it continues after that run's start has been rotated away.
const CONTINUED: &str = "daemon.log.continued";

/// The last record the daemon that last served `socket` wrote, when it ended without saying so:
/// killed, crashed, or the machine went down under it. None when it stopped or failed in its own
/// words, when the log does not say which daemon served, or when there is no log.
///
/// Read before this run's first record, by the daemon that holds the socket's lock.
pub(crate) fn unended(socket: &Path) -> Option<LastRecord> {
    let path = path_for(socket);
    let mut rotated = path.clone().into_os_string();
    rotated.push(".1");
    let ending = [path, PathBuf::from(rotated)]
        .iter()
        .find_map(|file| ending_in(&String::from_utf8_lossy(&std::fs::read(file).ok()?)));
    match ending {
        Some(Ending::Unsaid(last)) => Some(last),
        Some(Ending::Said | Ending::Unknown) | None => None,
    }
}

/// How the daemon that last served ended, as far as a log says.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Said,
    Unsaid(LastRecord),
    /// No record says which daemon served: a log from before [`CONTINUED`] was written, rotated
    /// past its run's start. Saying nothing beats sending somebody after a crash that was a stop.
    Unknown,
}

/// How the daemon that last served ended, by one file's text, or None for a file with no record
/// in it, which says nothing either way.
///
/// The daemon that served is the one that wrote the newest [`STARTED`], [`SERVING`] or
/// [`CONTINUED`], and it ended in its own words when a [`STOPPED`] or [`FAILED`] after that is
/// its own, matched by pid. Any other daemon's ending is not: a successor that failed to take
/// over leaves its `daemon.failed` in this log, and so does a start that failed before serving.
fn ending_in(log: &str) -> Option<Ending> {
    let mut ended = Vec::new();
    let mut newest: Vec<(i32, LastRecord)> = Vec::new();
    for line in log.lines().rev() {
        let Ok(Logged { time, pid, event }) = serde_json::from_str(line) else { continue };
        if !newest.iter().any(|(by, _)| *by == pid) {
            newest.push((pid, LastRecord { time, event: event.clone() }));
        }
        match event.as_str() {
            STOPPED | FAILED => ended.push(pid),
            STARTED | SERVING | CONTINUED if ended.contains(&pid) => return Some(Ending::Said),
            STARTED | SERVING | CONTINUED => {
                let last = newest.into_iter().find(|(by, _)| *by == pid).map(|(_, last)| last);
                return last.map(Ending::Unsaid);
            }
            _ => {}
        }
    }
    (!newest.is_empty()).then_some(Ending::Unknown)
}

/// The log of this process, for the records written whatever its level.
static LOG: OnceLock<Arc<DaemonLog>> = OnceLock::new();

/// Logs `event`, one of a run's lifecycle records, at info and whatever `MUSTER_LOG_LEVEL` says:
/// the next daemon reads them to tell a stop from a kill ([`unended`]), and at a level above
/// info a stop would otherwise read as one.
pub(crate) fn lifecycle(event: &str, fields: BTreeMap<String, String>) {
    match LOG.get() {
        Some(log) => log.write(&this_process(event, fields)),
        None => log::info(event, fields),
    }
}

/// Logs `event` as [`lifecycle`] does, and writes nothing after it: the daemon's word that it
/// stopped, which has to be the last record of its run whatever a thread still running logs. In
/// one hold of the log's lock, since a record written between the two could rotate the log and
/// leave the new file saying this daemon's run goes on.
pub(crate) fn last(event: &str, fields: BTreeMap<String, String>) {
    match LOG.get() {
        Some(log) => log.write_then(&this_process(event, fields), true),
        None => log::info(event, fields),
    }
}

fn this_process(event: &str, fields: BTreeMap<String, String>) -> LogRecord {
    let pid = i32::try_from(std::process::id()).unwrap_or_default();
    LogRecord::now(LogLevel::Info, PROCESS, pid, event, fields)
}

/// What every record says its process is.
const PROCESS: &str = "daemon";

#[derive(Debug)]
pub(crate) struct DaemonLog {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    file: Rotating,
    /// The last [`KEPT`] records, by number.
    kept: VecDeque<(u64, Arc<str>)>,
    next: u64,
    followers: Vec<Outbox>,
    /// The first record kept from the file while another daemon writes it: from a handoff's
    /// commit on the daemon handing over, and until it on the daemon taking over.
    withheld_from: Option<u64>,
    /// Where a daemon taking over writes each record until its commit ([`handed_over_path`]),
    /// so a daemon that fails before then leaves them for the one handing over to take in.
    handing_over: Option<File>,
    /// The daemon has said it stopped ([`last`]).
    sealed: bool,
}

/// Beside the log's file: `<name>.log.handoff`.
fn handed_over_path(log: &Path) -> PathBuf {
    let mut path = log.as_os_str().to_owned();
    path.push(".handoff");
    PathBuf::from(path)
}

impl DaemonLog {
    /// Starts the log beside the socket and sends every record of this process to it, unless
    /// `MUSTER_LOG=0` turns logging off. `MUSTER_LOG_LEVEL` sets the level, as for every Muster
    /// process. Called once the daemon holds its socket's lock, which makes it the file's only
    /// writer - or, with `withheld`, by a daemon taking over from the one that does: its records
    /// stay out of the file until [`DaemonLog::write_file`].
    pub(crate) fn start(socket: &Path, withheld: bool) -> Option<Arc<DaemonLog>> {
        if std::env::var("MUSTER_LOG").as_deref() == Ok("0") {
            return None;
        }
        let level = std::env::var("MUSTER_LOG_LEVEL")
            .ok()
            .and_then(|name| LogLevel::parse(&name))
            .unwrap_or(LogLevel::Debug);
        let file = if withheld {
            Rotating::closed(path_for(socket), FILE_BYTES)
        } else {
            Rotating::open(path_for(socket), FILE_BYTES)
        };
        let log = Arc::new(DaemonLog::new(file));
        if withheld {
            log.withhold_file();
            let mut inner = log.inner();
            inner.handing_over = File::options()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(handed_over_path(&inner.file.path))
                .ok();
        }
        let _ = LOG.set(Arc::clone(&log));
        log::install(Box::new(Sink(Arc::clone(&log))), PROCESS, level);
        Some(log)
    }

    fn new(file: Rotating) -> DaemonLog {
        DaemonLog {
            inner: Mutex::new(Inner {
                file,
                kept: VecDeque::with_capacity(KEPT),
                next: 1,
                followers: Vec::new(),
                withheld_from: None,
                handing_over: None,
                sealed: false,
            }),
        }
    }

    /// Recovered rather than reported when poisoned: reporting it would log, which is this.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, record: &LogRecord) {
        self.write_then(record, false);
    }

    /// Writes `record`, and with `seal` nothing after it.
    fn write_then(&self, record: &LogRecord, seal: bool) {
        let line = sink::line(record);
        let mut inner = self.inner();
        if inner.sealed {
            return;
        }
        inner.sealed = seal;
        if inner.withheld_from.is_none() {
            inner.file.append(line.as_bytes());
        } else if let Some(file) = &mut inner.handing_over {
            let _ = file.write_all(line.as_bytes());
        }
        let number = inner.next;
        inner.next += 1;
        let line: Arc<str> = line.trim_end_matches('\n').into();
        if !inner.followers.is_empty() {
            let frame = frame(number, &line);
            inner.followers.retain(|follower| follower.offer(Arc::clone(&frame)));
        }
        if inner.kept.len() == KEPT {
            inner.kept.pop_front();
        }
        inner.kept.push_back((number, line));
    }

    /// Hands `outbox` every record held after `after` (all of them when it is absent), then every
    /// record from now on. Returns the oldest record held and the newest written.
    pub(crate) fn follow(&self, outbox: &Outbox, after: Option<u64>) -> (u64, u64) {
        let mut inner = self.inner();
        for (number, line) in inner.replay(after) {
            if !outbox.offer(frame(number, &line)) {
                return inner.span();
            }
        }
        if !inner.followers.iter().any(|follower| follower.id == outbox.id) {
            inner.followers.push(outbox.clone());
        }
        inner.span()
    }

    /// Keeps records out of the file, which the daemon taking over writes from its commit.
    pub(crate) fn withhold_file(&self) {
        let mut inner = self.inner();
        inner.file.file = None;
        if inner.withheld_from.is_none() {
            inner.withheld_from = Some(inner.next);
        }
    }

    /// Writes to the file again, first the records kept from it that are still held.
    pub(crate) fn write_file(&self) {
        let mut inner = self.inner();
        let Some(from) = inner.withheld_from.take() else { return };
        let withheld: Vec<Arc<str>> = inner
            .kept
            .iter()
            .filter(|(number, _)| *number >= from)
            .map(|(_, line)| Arc::clone(line))
            .collect();
        for line in withheld {
            inner.file.append(format!("{line}\n").as_bytes());
        }
        if inner.handing_over.take().is_some() {
            let _ = std::fs::remove_file(handed_over_path(&inner.file.path));
        }
    }

    /// Removes what a daemon taking over left behind in an earlier handoff, before starting
    /// another: one whose old daemon was killed before it could take the records in, which a
    /// successor that never started would otherwise pass off as its own.
    pub(crate) fn forget_handed_over(&self) {
        let _ = std::fs::remove_file(handed_over_path(&self.inner().file.path));
    }

    /// Writes into the file what a daemon that failed to take over logged before it failed,
    /// and removes where it left it. Returns how many records that was.
    pub(crate) fn take_in_handed_over(&self) -> usize {
        let mut inner = self.inner();
        let path = handed_over_path(&inner.file.path);
        let Ok(records) = std::fs::read_to_string(&path) else { return 0 };
        let _ = std::fs::remove_file(&path);
        let mut count = 0;
        for line in records.lines().filter(|line| !line.is_empty()) {
            inner.file.append(format!("{line}\n").as_bytes());
            count += 1;
        }
        count
    }

    /// Stops handing records to a connection that has gone.
    pub(crate) fn unfollow(&self, connection: u64) {
        self.inner().followers.retain(|follower| follower.id != connection);
    }
}

impl Inner {
    fn replay(&self, after: Option<u64>) -> Vec<(u64, Arc<str>)> {
        self.kept
            .iter()
            .filter(|(number, _)| after.is_none_or(|after| *number > after))
            .cloned()
            .collect()
    }

    fn span(&self) -> (u64, u64) {
        (self.kept.front().map_or(self.next, |(number, _)| *number), self.next - 1)
    }
}

fn frame(number: u64, line: &str) -> Arc<[u8]> {
    proto::ControlMessage {
        message: Some(proto::control_message::Message::LogLine(proto::LogLine {
            number,
            line: line.to_string(),
        })),
    }
    .encode_to_vec()
    .into()
}

struct Sink(Arc<DaemonLog>);

impl LogSink for Sink {
    fn write(&self, record: &LogRecord) {
        self.0.write(record);
    }
}

/// A file the daemon appends to and rotates itself once it passes its limit. Nothing else
/// holds it open, so a rename is all a rotation needs.
#[derive(Debug)]
struct Rotating {
    path: PathBuf,
    /// None when it could not be opened: the records still reach followers.
    file: Option<File>,
    size: u64,
    limit: u64,
}

impl Rotating {
    fn open(path: PathBuf, limit: u64) -> Rotating {
        let mut rotating = Rotating::closed(path, limit);
        rotating.reopen();
        rotating
    }

    /// Opened at the first record appended.
    fn closed(path: PathBuf, limit: u64) -> Rotating {
        Rotating { path, file: None, size: 0, limit }
    }

    /// Opens the file again, at the length it has: after a rotation that failed that is the
    /// full file, and counting it as empty would let it grow by another limit each time.
    fn reopen(&mut self) {
        self.file = append_to(&self.path).ok();
        self.size =
            self.file.as_ref().and_then(|file| file.metadata().ok()).map_or(0, |meta| meta.len());
    }

    /// Nowhere to report that any of this failed: it would be a record in this same file. So a
    /// failure costs records and never the bound. A file that would not open is tried again at
    /// the next record, and one that could not be moved aside is started over.
    fn append(&mut self, line: &[u8]) {
        let length = line.len() as u64;
        if self.file.is_none() {
            self.reopen();
        }
        if self.size > 0 && self.size + length > self.limit {
            let mut previous = self.path.as_os_str().to_owned();
            previous.push(".1");
            if std::fs::rename(&self.path, previous).is_err() {
                let _ = File::options().write(true).truncate(true).open(&self.path);
            }
            self.reopen();
            if self.size > 0 && self.size + length > self.limit {
                return;
            }
            let continued = sink::line(&this_process(CONTINUED, BTreeMap::new()));
            if let Some(file) = &mut self.file
                && file.write_all(continued.as_bytes()).is_ok()
            {
                self.size += continued.len() as u64;
            }
        }
        if let Some(file) = &mut self.file
            && file.write_all(line).is_ok()
        {
            self.size += length;
        }
    }
}

/// Opened close-on-exec, as std opens every file, so no pane's program inherits it.
fn append_to(path: &Path) -> std::io::Result<File> {
    File::options().create(true).append(true).mode(0o600).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("muster-log-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(event: &str) -> LogRecord {
        LogRecord::now(LogLevel::Info, "daemon", 1, event, BTreeMap::new())
    }

    /// A log's text: each record the event `pid` logged.
    fn by(records: &[(i32, &str)]) -> String {
        let line = |&(pid, event): &(i32, &str)| {
            sink::line(&LogRecord::now(LogLevel::Info, "daemon", pid, event, BTreeMap::new()))
        };
        records.iter().map(line).collect()
    }

    /// A successor that fails to take over leaves its records, its `daemon.failed` among them,
    /// in the log of the daemon that goes on serving.
    #[test]
    fn a_failed_successors_ending_is_not_the_serving_daemons() {
        let log = by(&[
            (1, "daemon.started"),
            (2, "daemon.handoff.taking_over"),
            (2, "daemon.failed"),
            (1, "daemon.handoff.failed"),
            (1, "test.last"),
        ]);
        assert_eq!(unsaid(&log), "test.last");
    }

    /// A daemon that failed before it served says nothing of the one killed before it.
    #[test]
    fn a_start_that_failed_does_not_end_the_run_before_it() {
        let log = by(&[(1, "daemon.started"), (1, "test.last"), (3, "daemon.failed")]);
        assert_eq!(unsaid(&log), "test.last");
    }

    fn lines(events: &[&str]) -> String {
        events.iter().map(|event| sink::line(&record(event))).collect()
    }

    fn unsaid(log: &str) -> String {
        match ending_in(log) {
            Some(Ending::Unsaid(last)) => last.event,
            other => panic!("expected a run that never said it ended, got {other:?}"),
        }
    }

    #[test]
    fn a_log_whose_last_run_wrote_no_ending_is_unended_and_names_its_last_record() {
        let killed = lines(&[STARTED, STOPPED, STARTED, "test.last"]);
        assert_eq!(unsaid(&killed), "test.last");
        assert_eq!(unsaid(&lines(&[STARTED])), STARTED);
        // A line cut short by the kill is no record.
        let torn = format!("{killed}{{\"time\":\"2026-11");
        assert_eq!(unsaid(&torn), "test.last");
    }

    #[test]
    fn a_log_whose_last_run_stopped_or_failed_is_ended() {
        assert_eq!(ending_in(&lines(&[STARTED, "test.work", STOPPED])), Some(Ending::Said));
        assert_eq!(ending_in(&lines(&[STARTED, FAILED])), Some(Ending::Said));
        assert_eq!(ending_in(""), None, "an empty file says nothing either way");
    }

    /// The daemon that took the panes over is the one whose ending counts, and the one that
    /// handed them over never says it stopped.
    #[test]
    fn after_a_handoff_the_successors_ending_is_the_one_read() {
        let handed = [(1, STARTED), (1, "daemon.handoff.committed"), (2, SERVING)];
        let stopped = by(&[&handed[..], &[(2, STOPPED)]].concat());
        assert_eq!(ending_in(&stopped), Some(Ending::Said));
        let killed = by(&[&handed[..], &[(2, "test.last")]].concat());
        assert_eq!(unsaid(&killed), "test.last");
    }

    /// A file the log was rotated into opens with whose run it continues, so a run older than
    /// both files still has its ending told apart. A file with no such record cannot say.
    #[test]
    fn a_run_rotated_past_its_start_is_read_from_the_record_its_file_opens_with() {
        assert_eq!(unsaid(&lines(&[CONTINUED, "test.last"])), "test.last");
        assert_eq!(ending_in(&lines(&[CONTINUED, "test.work", STOPPED])), Some(Ending::Said));
        assert_eq!(ending_in(&lines(&["test.first", "test.last"])), Some(Ending::Unknown));
        assert_eq!(ending_in(&lines(&["test.work", STOPPED])), Some(Ending::Unknown));
    }

    #[test]
    fn a_file_the_log_is_rotated_into_opens_by_saying_whose_run_it_continues() {
        let scratch = Scratch::new("continued");
        let path = scratch.0.join("daemon.log");
        let log = DaemonLog::new(Rotating::open(path.clone(), 2000));
        for index in 0..100 {
            log.write(&record(&format!("test.record.{index}")));
        }
        let current = std::fs::read_to_string(&path).unwrap();
        let first = current.lines().next().unwrap();
        assert!(first.contains(CONTINUED), "{first}");
        assert!(first.contains(&format!("\"pid\":{}", std::process::id())), "{first}");
        assert!(matches!(ending_in(&current), Some(Ending::Unsaid(_))));
    }

    #[test]
    fn nothing_is_written_after_the_record_that_seals_the_log() {
        let scratch = Scratch::new("sealed");
        let path = scratch.0.join("daemon.log");
        let log = DaemonLog::new(Rotating::open(path.clone(), FILE_BYTES));
        log.write_then(&record(STOPPED), true);
        log.write(&record("test.straggler"));
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(file.contains(STOPPED) && !file.contains("test.straggler"), "{file}");
    }

    #[test]
    fn the_rotated_file_answers_when_the_current_one_holds_no_record() {
        let scratch = Scratch::new("unended");
        let socket = scratch.0.join("daemon.sock");
        assert_eq!(unended(&socket), None, "no log at all");
        std::fs::write(scratch.0.join("daemon.log.1"), lines(&["daemon.started"])).unwrap();
        assert!(unended(&socket).is_some());
        std::fs::write(scratch.0.join("daemon.log"), lines(&["daemon.stopped"])).unwrap();
        assert_eq!(unended(&socket), None);
    }

    #[test]
    fn the_file_is_rotated_before_it_passes_its_limit_and_keeps_the_newest_records() {
        let scratch = Scratch::new("rotate");
        let path = scratch.0.join("daemon.log");
        let log = DaemonLog::new(Rotating::open(path.clone(), 2000));
        for index in 0..100 {
            log.write(&record(&format!("test.record.{index}")));
        }
        let current = std::fs::read_to_string(&path).unwrap();
        let previous = std::fs::read_to_string(scratch.0.join("daemon.log.1")).unwrap();
        assert!(current.len() <= 2000 && previous.len() <= 2000);
        assert!(current.contains("test.record.99\""), "the newest record is in the file");
        assert!(!previous.contains("test.record.0\""), "the oldest is gone");
        assert!(current.lines().chain(previous.lines()).all(|line| line.ends_with('}')));
    }

    #[test]
    fn records_left_by_an_earlier_handoff_are_not_taken_in_by_a_later_one() {
        let scratch = Scratch::new("stale");
        let path = scratch.0.join("daemon.log");
        std::fs::write(handed_over_path(&path), "{\"event\":\"stale\"}\n").unwrap();
        let log = DaemonLog::new(Rotating::open(path.clone(), 2000));
        log.forget_handed_over();
        assert_eq!(log.take_in_handed_over(), 0);
    }

    #[test]
    fn a_file_that_cannot_be_moved_aside_is_started_over_rather_than_grown() {
        let scratch = Scratch::new("stuck");
        let path = scratch.0.join("daemon.log");
        // A directory with something in it, which no rename replaces.
        std::fs::create_dir_all(scratch.0.join("daemon.log.1/in-the-way")).unwrap();
        let log = DaemonLog::new(Rotating::open(path.clone(), 2000));
        for index in 0..100 {
            log.write(&record(&format!("test.record.{index}")));
        }
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.len() <= 2000, "{} bytes past a 2000-byte limit", current.len());
        assert!(current.contains("test.record.99\""), "the newest record is in the file");
    }

    #[test]
    fn a_file_that_would_not_open_is_tried_again_at_the_next_record() {
        let scratch = Scratch::new("reopen");
        let path = scratch.0.join("later/daemon.log");
        let log = DaemonLog::new(Rotating::open(path.clone(), FILE_BYTES));
        log.write(&record("test.record.lost"));
        std::fs::create_dir_all(scratch.0.join("later")).unwrap();
        log.write(&record("test.record.kept"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("test.record.kept"));
    }

    #[test]
    fn a_follower_is_replayed_what_it_has_not_seen_of_the_last_records() {
        let scratch = Scratch::new("replay");
        let log = DaemonLog::new(Rotating::open(scratch.0.join("daemon.log"), FILE_BYTES));
        for index in 0..KEPT + 5 {
            log.write(&record(&format!("test.record.{index}")));
        }
        let inner = log.inner();
        assert_eq!(inner.span(), (6, (KEPT + 5) as u64), "the first five have left memory");
        let all = inner.replay(None);
        assert_eq!(all.len(), KEPT);
        assert!(all[0].1.contains("test.record.5\""));
        let later: Vec<u64> = inner.replay(Some(1000)).iter().map(|(number, _)| *number).collect();
        assert_eq!(later, vec![1001, 1002, 1003, 1004, 1005]);
    }

    /// Two daemons share the file across a handoff and never write it at once: the one handing
    /// over stops at its commit, the one taking over starts there, and each writes what it kept
    /// meanwhile when it takes the file back.
    #[test]
    fn records_withheld_from_the_file_reach_it_in_order_once_it_is_written_again() {
        let scratch = Scratch::new("withheld");
        let path = scratch.0.join("daemon.log");
        let log = DaemonLog::new(Rotating::open(path.clone(), FILE_BYTES));
        log.write(&record("test.record.before"));
        log.withhold_file();
        log.write(&record("test.record.while"));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("test.record.while"));
        log.write_file();
        log.write(&record("test.record.after"));
        let file = std::fs::read_to_string(&path).unwrap();
        let events: Vec<&str> = ["before", "while", "after"]
            .into_iter()
            .filter(|event| file.contains(&format!("test.record.{event}\"")))
            .collect();
        assert_eq!(events, ["before", "while", "after"]);
        let at = |event: &str| file.find(&format!("test.record.{event}\"")).unwrap();
        assert!(at("before") < at("while") && at("while") < at("after"));
    }
}
