//! What a daemon restart needs, kept on disk (MIP-3 section 2): the shape of every tab, each
//! pane's name and directory, and the settings an app last gave. Never a title, an agent's
//! state or facts, a command or whether a process is alive: those are observations, and a
//! restarted daemon observes them afresh.
//!
//! One JSON file beside the daemon's socket, written whole to a temporary file, synced and
//! renamed over the last, so a crash at any point leaves either the old file or the new one. The
//! write happens on a thread of its own, a second after the first change it covers, with the
//! session locked only long enough to copy the state out.
//!
//! **The format outlives the code that wrote it**, so changing it follows rules, and
//! `tests/fixtures/state-v1.json` holds this code to them:
//! - A field is added only with `#[serde(default)]`, so a file written before it existed still
//!   reads. A field every file has is required, and its absence means a damaged file.
//! - A field is never renamed or retyped. The settings are the protocol's own messages, keyed by
//!   their field names, so renaming a `Settings` field - which the protocol allows - needs a
//!   `#[serde(alias)]` naming the old one, added in `muster-daemon-proto/build.rs`. The
//!   protocol's enums are stored as their numbers, which a renamed value leaves alone; a split's
//!   axis is stored as a word, and follows the rule for fields.
//! - Anything else bumps [`VERSION`], and [`load`] goes on reading every earlier version.
//! - A fixture is frozen once written. A new version gets a fixture of its own, and so does a
//!   setting added since the last fixture: the tests' `FIXTURES` says which fixture holds which
//!   setting, and fails until every setting is held by one.

use std::collections::HashSet;
use std::fs::File;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use serde::{Deserialize, Serialize};

use crate::pty::Grid;
use crate::session::{Shared, valid_name, valid_ratio};
use crate::tree::Node;

/// The file's format. A daemon reads every version up to its own and refuses a newer one.
pub(crate) const VERSION: u32 = 1;

/// How long after a change the state is written. Later changes do not push it back, so a pane
/// retitling itself every second cannot put the write off forever.
const DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct State {
    pub(crate) version: u32,
    /// The daemon that wrote it, for a person reading the file.
    pub(crate) daemon: String,
    pub(crate) settings: proto::Settings,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) panes: Vec<Pane>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tab {
    #[serde(rename = "tab")]
    pub(crate) name: String,
    pub(crate) label: proto::Label,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zoomed: Option<String>,
    pub(crate) root: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pane {
    #[serde(rename = "pane")]
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) cwd: PathBuf,
    /// The size it was last shown at, so its shell starts at that size.
    pub(crate) grid: Grid,
}

/// The file beside a daemon's socket: `~/.muster/daemon/<install>.state.json`.
pub(crate) fn path_for(socket: &Path) -> PathBuf {
    socket.with_extension("state.json")
}

/// What a daemon found where its state is kept.
#[derive(Debug, PartialEq)]
pub(crate) enum Loaded {
    Nothing,
    // Boxed: a state is several hundred bytes, and every other answer a few.
    State(Box<State>),
    /// Written by a newer daemon, in a format this one cannot read.
    Newer(u32),
    /// Not a state this daemon could have written.
    Corrupt(String),
    /// There, and not readable.
    Unreadable(String),
}

pub(crate) fn load(path: &Path) -> Loaded {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Loaded::Nothing,
        Err(error) => Loaded::Unreadable(error.to_string()),
    }
}

/// A state as the file holds it, from wherever it came: a daemon handing its panes over sends
/// the state in the same format, under the same rules.
pub(crate) fn parse(bytes: &[u8]) -> Loaded {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }
    // The version first, on its own: a newer daemon's file may not parse as this one's at all,
    // and must still be told apart from a damaged one.
    let version = match serde_json::from_slice::<Versioned>(bytes) {
        Ok(versioned) => versioned.version,
        Err(error) => return Loaded::Corrupt(error.to_string()),
    };
    if version > VERSION {
        return Loaded::Newer(version);
    }
    match serde_json::from_slice::<State>(bytes) {
        Ok(state) => match validate(&state) {
            Ok(()) => Loaded::State(Box::new(state)),
            Err(why) => Loaded::Corrupt(why),
        },
        Err(error) => Loaded::Corrupt(error.to_string()),
    }
}

/// Holds a state to what this daemon could have written, so that restoring it cannot build a
/// tab this daemon's requests could not.
fn validate(state: &State) -> Result<(), String> {
    let mut tabs = HashSet::new();
    let mut placed = HashSet::new();
    for tab in &state.tabs {
        valid_name("tab", &tab.name)?;
        if !tabs.insert(&tab.name) {
            return Err(format!("tab {} is recorded twice", tab.name));
        }
        let panes = tab.root.panes();
        for pane in &panes {
            valid_name("pane", pane)?;
            if !placed.insert(pane.to_string()) {
                return Err(format!("pane {pane} is in two places"));
            }
        }
        ratios(&tab.root)?;
        if let Some(zoomed) = &tab.zoomed
            && !tab.root.contains(zoomed)
        {
            return Err(format!("tab {} zooms {zoomed}, which it does not hold", tab.name));
        }
    }
    let mut recorded = HashSet::new();
    for pane in &state.panes {
        if !recorded.insert(pane.name.as_str()) {
            return Err(format!("pane {} is recorded twice", pane.name));
        }
        if !placed.contains(&pane.name) {
            return Err(format!("pane {} is in no tab", pane.name));
        }
        if pane.grid.cols == 0 || pane.grid.rows == 0 {
            return Err(format!("pane {} has no cells", pane.name));
        }
    }
    if let Some(missing) = placed.iter().find(|pane| !recorded.contains(pane.as_str())) {
        return Err(format!("pane {missing} is in a tab and not recorded"));
    }
    if state.settings.palette.as_ref().is_some_and(|palette| palette.entries.len() > 256) {
        return Err("the palette has more than 256 entries".to_string());
    }
    Ok(())
}

fn ratios(node: &Node) -> Result<(), String> {
    if let Node::Split { ratio, first, second, .. } = node {
        valid_ratio(*ratio)?;
        ratios(first)?;
        ratios(second)?;
    }
    Ok(())
}

/// Moves a file this daemon cannot use out of the way, keeping it, and says where it went.
pub(crate) fn move_aside(path: &Path) -> std::io::Result<PathBuf> {
    let aside = aside(path, "corrupt");
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// Copies the file at `path` beside itself, as `<file>.unrestored-<seconds>`: what a previous
/// run saved, kept for a person when this run could not bring all of it back and its next write
/// will leave the rest out.
pub(crate) fn keep_aside(path: &Path) -> std::io::Result<PathBuf> {
    let aside = aside(path, "unrestored");
    copy_as(path, &aside)?;
    Ok(aside)
}

/// Copies `path` to `copy` under another name first and renames it, so no copy is ever found
/// half written, and none is left behind when either step fails.
fn copy_as(path: &Path, copy: &Path) -> std::io::Result<()> {
    let mut copying = path.as_os_str().to_owned();
    copying.push(".copying");
    let copied = std::fs::copy(path, &copying).and_then(|_| std::fs::rename(&copying, copy));
    if copied.is_err() {
        let _ = std::fs::remove_file(&copying);
    }
    copied
}

fn aside(path: &Path, why: &str) -> PathBuf {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".{why}-{seconds}"));
    PathBuf::from(aside)
}

fn temporary(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

/// Writes `bytes` as the file at `path`, so that at every moment the path holds either what it
/// held before or all of `bytes`.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_temporary(path, bytes)?;
    commit(path)
}

/// Writes `bytes` as the file at `path`, whole or not at all, as [`write`] does, without asking
/// the disk to have it before returning: no full flush of the file, and none of its directory.
///
/// For state that is written often and costs little to lose its latest change to - after a power
/// loss the path holds an earlier whole file rather than this one. What a session's tabs are
/// keeps [`write`], since that is somebody's work. The message service's state is written on
/// every post and read, and its latest change lost costs a message shown as unread again, a wake
/// repeated, or a session joined that moment joining again; a group's policy and pause are read
/// back from its log, which is appended and synced first.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(temporary(path))?;
    file.write_all(bytes)?;
    file.sync_data()?;
    std::fs::rename(temporary(path), path)
}

fn write_temporary(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(temporary(path))?;
    file.write_all(bytes)?;
    // To the disk itself, before the rename can make it the file: F_FULLFSYNC on macOS.
    file.sync_all()
}

fn commit(path: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary(path), path)?;
    // The rename is an entry in the directory, which has to reach the disk too.
    File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
}

/// Writes the session's state a second after it changes, and once more as the daemon stops.
#[derive(Debug)]
pub(crate) struct Persister {
    path: PathBuf,
    pending: Mutex<Pending>,
    woken: Condvar,
    /// Every state written, for tests to see what the file held along the way.
    #[cfg(test)]
    writes: Mutex<Vec<State>>,
}

/// What the persist thread took out of the session.
pub(crate) enum Copied {
    State(Box<State>),
    /// The daemon has begun to stop, and has handed over what to leave behind.
    Stopping,
    /// The session is gone.
    Gone,
}

#[derive(Debug)]
struct Pending {
    phase: Phase,
    /// When the first change not yet written happened.
    changed: Option<Instant>,
    /// The state as the daemon began to stop, to write before it exits.
    last: Option<State>,
    /// A copy or a write is under way.
    writing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Saved tabs are coming back. Nothing is written: until they are all back the session
    /// holds less than the file, and a write would lose the difference.
    Restoring,
    Writing,
    /// The daemon is handing its panes to another, which writes the file once it serves.
    Paused,
    /// The daemon is stopping, and its last state is being written.
    Stopping,
    Stopped,
    /// Never writes: the file there is not this daemon's to replace.
    Off,
}

impl Persister {
    /// A persister for the file at `path`, writing nothing until [`Persister::arm`], and nothing
    /// ever if `disabled`.
    pub(crate) fn new(path: PathBuf, disabled: bool) -> Arc<Persister> {
        let phase = if disabled { Phase::Off } else { Phase::Restoring };
        Arc::new(Persister {
            path,
            pending: Mutex::new(Pending { phase, changed: None, last: None, writing: false }),
            woken: Condvar::new(),
            #[cfg(test)]
            writes: Mutex::new(Vec::new()),
        })
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        poison::lock(&self.pending, "daemon.persist")
    }

    /// Starts the thread that writes. Called with the session's `Shared`, which it locks only
    /// to copy the state out.
    pub(crate) fn start(self: &Arc<Persister>, shared: Weak<Shared>) {
        if self.pending().phase == Phase::Off {
            return;
        }
        let persister = Arc::clone(self);
        let started = std::thread::Builder::new().name("persist".to_string()).spawn(move || {
            persister.run(|| match shared.upgrade() {
                None => Copied::Gone,
                Some(shared) => shared
                    .lock()
                    .persisted_unless_stopping()
                    .map_or(Copied::Stopping, |state| Copied::State(Box::new(state))),
            });
        });
        if let Err(error) = started {
            // Nothing will ever write, so a stop must not wait for a write.
            self.pending().phase = Phase::Off;
            log::error(
                "daemon.state.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "nothing this daemon holds is written down, so a restart brings \
                                 back what the file held when it started",
                    "check" => "whether the daemon is out of threads",
                },
            );
        }
    }

    /// Something a restart would need changed.
    pub(crate) fn changed(&self) {
        let mut pending = self.pending();
        if pending.changed.is_none() {
            pending.changed = Some(Instant::now());
            self.woken.notify_all();
        }
    }

    /// Restoring is over, so the session now holds everything the file did.
    pub(crate) fn arm(&self) {
        let mut pending = self.pending();
        if pending.phase == Phase::Restoring {
            pending.phase = Phase::Writing;
            self.woken.notify_all();
        }
    }

    /// Writes nothing until [`Persister::resume`], once any write under way has finished: the
    /// daemon taking over writes the file from when it serves. False when that write was still
    /// under way after `within`, and the file may yet be written by this daemon.
    pub(crate) fn pause(&self, within: Duration) -> bool {
        let mut pending = self.pending();
        if pending.phase != Phase::Writing {
            return true;
        }
        pending.phase = Phase::Paused;
        let (_pending, waited) = self
            .woken
            .wait_timeout_while(pending, within, |pending| pending.writing)
            .unwrap_or_else(PoisonError::into_inner);
        !waited.timed_out()
    }

    /// Writes again after a handoff that failed, and writes what changed meanwhile.
    pub(crate) fn resume(&self) {
        let mut pending = self.pending();
        if pending.phase == Phase::Paused {
            pending.phase = Phase::Writing;
            pending.changed.get_or_insert_with(Instant::now);
            self.woken.notify_all();
        }
    }

    /// Writes nothing more this run, since the file holds something this daemon could not
    /// bring back and could not keep elsewhere.
    pub(crate) fn off(&self) {
        let mut pending = self.pending();
        pending.phase = Phase::Off;
        self.woken.notify_all();
    }

    /// False once this persister will never write again this run, as one that is off.
    pub(crate) fn saving(&self) -> bool {
        self.pending().phase != Phase::Off
    }

    /// The file this persister writes.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The daemon is stopping, and this is the state to leave behind: what it held before it
    /// closed anything. Only the first call counts.
    pub(crate) fn stopping(&self, state: State) {
        let mut pending = self.pending();
        match pending.phase {
            Phase::Writing => {
                pending.phase = Phase::Stopping;
                pending.last = Some(state);
            }
            Phase::Restoring | Phase::Paused => {
                pending.phase = Phase::Stopped;
                log::warn(
                    "daemon.state.not_saved",
                    fields! {
                        "path" => self.path.display(),
                        "impact" => "the daemon stopped before every saved tab was back, or while \
                                     handing its panes over, so it saves nothing; the file keeps \
                                     what was last saved, and loses what changed since",
                        "check" => "a daemon.state.* record before this one, if restoring was \
                                    stuck or failed",
                    },
                );
            }
            Phase::Stopping | Phase::Stopped | Phase::Off => return,
        }
        self.woken.notify_all();
    }

    /// Waits for the state handed to [`Persister::stopping`] to be written, at most `within`.
    pub(crate) fn wait_until_stopped(&self, within: Duration) {
        let pending = self.pending();
        let _ = self
            .woken
            .wait_timeout_while(pending, within, |pending| pending.phase == Phase::Stopping);
    }

    /// Writes whatever `copy` takes out of the session, a second after each change, until the
    /// daemon stops.
    fn run(&self, copy: impl Fn() -> Copied) {
        let mut written = Vec::new();
        let mut pending = self.pending();
        loop {
            match pending.phase {
                Phase::Stopping => {
                    let last = pending.last.take();
                    drop(pending);
                    if let Some(last) = last {
                        self.write_if_changed(&last, &mut written);
                    }
                    self.pending().phase = Phase::Stopped;
                    self.woken.notify_all();
                    return;
                }
                Phase::Stopped | Phase::Off => return,
                Phase::Restoring | Phase::Paused => {
                    pending = self.woken.wait(pending).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
                Phase::Writing => {}
            }
            let Some(due) = pending.changed.map(|changed| changed + DELAY) else {
                pending = self.woken.wait(pending).unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let now = Instant::now();
            if now < due {
                pending = self
                    .woken
                    .wait_timeout(pending, due - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            pending.changed = None;
            pending.writing = true;
            drop(pending);
            let copied = copy();
            if let Copied::State(state) = &copied {
                self.write_if_changed(state, &mut written);
            }
            pending = self.pending();
            pending.writing = false;
            self.woken.notify_all();
            match copied {
                // What the daemon held before it began to stop is the state to leave, and it
                // was handed over in the same hold of the session's lock that this copy saw.
                Copied::State(_) | Copied::Stopping => {}
                Copied::Gone => return,
            }
        }
    }

    /// Writes `state` unless it is exactly what was written last: a title changing moves no
    /// byte of the file, and costs no write. Never a state this daemon's next start would refuse:
    /// that would lose every tab, where keeping the last file loses only the latest change.
    fn write_if_changed(&self, state: &State, written: &mut Vec<u8>) {
        if let Err(why) = validate(state) {
            log::error(
                "daemon.state.invalid",
                fields! {
                    "why" => why,
                    "impact" => "this change is not written down; the file keeps the last state \
                                 that was",
                    "check" => "this is a bug: the session holds a state its own restart would \
                                refuse",
                },
            );
            return;
        }
        let bytes = match serde_json::to_vec_pretty(state) {
            Ok(mut bytes) => {
                bytes.push(b'\n');
                bytes
            }
            Err(error) => {
                log::error(
                    "daemon.state.unencodable",
                    fields! {
                        "error" => error,
                        "impact" => "this change is not written down",
                        "check" => "this is a bug: every state the session holds should encode",
                    },
                );
                return;
            }
        };
        if bytes == *written {
            return;
        }
        match write(&self.path, &bytes) {
            Ok(()) => {
                *written = bytes;
                #[cfg(test)]
                poison::lock(&self.writes, "daemon.persist.writes").push(state.clone());
            }
            Err(error) => log::warn(
                "daemon.state.not_written",
                fields! {
                    "path" => self.path.display(),
                    "error" => error,
                    "impact" => "a restart before the next write succeeds brings back the tabs \
                                 as they were at the last one",
                    "check" => "whether the disk is full or the directory is writable; the next \
                                change tries again",
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Axis;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("muster-persist-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("daemon.state.json")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pane(name: &str) -> Pane {
        Pane {
            name: name.to_string(),
            label: None,
            cwd: PathBuf::from("/tmp"),
            grid: Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 },
        }
    }

    fn state() -> State {
        State {
            version: VERSION,
            daemon: "test".to_string(),
            settings: proto::Settings {
                palette: Some(proto::Palette { entries: vec![0x10_20_30], ..Default::default() }),
                ..Default::default()
            },
            tabs: vec![Tab {
                name: "t1".to_string(),
                label: proto::Label { text: Some("work".to_string()), generation: 2 },
                zoomed: Some("p2".to_string()),
                root: Node::Split {
                    axis: Axis::Columns,
                    ratio: 0.3,
                    first: Box::new(Node::Pane("p1".to_string())),
                    second: Box::new(Node::Pane("p2".to_string())),
                },
            }],
            panes: vec![Pane { label: Some("A".to_string()), ..pane("p1") }, pane("p2")],
        }
    }

    fn written(scratch: &Scratch, state: &State) {
        write(&scratch.file(), &serde_json::to_vec_pretty(state).unwrap()).unwrap();
    }

    #[test]
    fn a_state_reads_back_as_it_was_written() {
        let scratch = Scratch::new("round-trip");
        written(&scratch, &state());
        assert_eq!(load(&scratch.file()), Loaded::State(Box::new(state())));
    }

    #[test]
    fn a_write_cut_off_before_its_rename_leaves_the_last_state_readable() {
        let scratch = Scratch::new("cut-off");
        written(&scratch, &state());
        write_temporary(&scratch.file(), b"{\"version\": 1, \"tabs\": [{\"tab\"").unwrap();
        assert_eq!(load(&scratch.file()), Loaded::State(Box::new(state())));
        let mut next = state();
        next.tabs[0].label.generation = 3;
        written(&scratch, &next);
        assert_eq!(
            load(&scratch.file()),
            Loaded::State(Box::new(next)),
            "the leftover is written over"
        );
    }

    #[test]
    fn a_newer_daemons_file_is_told_apart_from_a_damaged_one() {
        let scratch = Scratch::new("newer");
        std::fs::write(scratch.file(), b"{\"version\": 7, \"something\": \"else entirely\"}")
            .unwrap();
        assert_eq!(load(&scratch.file()), Loaded::Newer(7));
        std::fs::write(scratch.file(), b"{\"version\": 1, \"tabs\": 3").unwrap();
        assert!(matches!(load(&scratch.file()), Loaded::Corrupt(_)));
        std::fs::write(scratch.file(), b"not json").unwrap();
        assert!(matches!(load(&scratch.file()), Loaded::Corrupt(_)));
        assert_eq!(load(&scratch.0.join("absent")), Loaded::Nothing);
    }

    /// The stop takes the session's lock to hand over what it held and close every tab; a write
    /// that fell due meanwhile gets the lock next and must not write the closed session, even
    /// for the moment before the handed-over state replaces it.
    #[test]
    fn a_write_due_as_the_daemon_stops_never_writes_the_closed_session() {
        let scratch = Scratch::new("stopping");
        let persister = Persister::new(scratch.file(), false);
        persister.arm();
        persister.changed();
        // The session: whether it has begun to stop, and what it holds.
        let session = Arc::new(Mutex::new((false, state())));
        let (writing, held) = (Arc::clone(&persister), Arc::clone(&session));
        let thread = std::thread::spawn(move || {
            writing.run(|| {
                let mut session = held.lock().unwrap();
                if !session.0 {
                    // The stop ran first: `close_everything`.
                    writing.stopping(session.1.clone());
                    session.0 = true;
                    session.1.tabs.clear();
                    session.1.panes.clear();
                }
                if session.0 {
                    Copied::Stopping
                } else {
                    Copied::State(Box::new(session.1.clone()))
                }
            });
        });
        thread.join().unwrap();
        assert_eq!(*persister.writes.lock().unwrap(), [state()], "only what it held");
    }

    #[test]
    fn a_copy_that_fails_leaves_nothing_half_written() {
        let dir = std::env::temp_dir().join(format!("muster-copy-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("in-the-way")).unwrap();
        std::fs::write(dir.join("in-the-way").join("file"), "").unwrap();
        let file = dir.join("state.json");
        std::fs::write(&file, "{}").unwrap();

        assert!(copy_as(&file, &dir.join("in-the-way")).is_err());
        assert!(!dir.join("state.json.copying").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pause_says_so_when_a_write_under_way_does_not_finish() {
        let persister = Persister::new(PathBuf::from("/nonexistent/state.json"), false);
        persister.arm();
        assert!(persister.pause(Duration::from_millis(10)), "nothing under way");
        persister.resume();
        persister.pending().writing = true;
        assert!(!persister.pause(Duration::from_millis(50)), "a write that did not finish");
    }

    /// While saved tabs are coming back the session holds less than the file, so a change
    /// long overdue writes nothing until restoring ends, and then writes at once.
    #[test]
    fn nothing_is_written_until_restoring_ends() {
        let scratch = Scratch::new("restoring");
        let persister = Persister::new(scratch.file(), false);
        persister.changed();
        persister.pending().changed = Instant::now().checked_sub(DELAY * 2);
        let writing = Arc::clone(&persister);
        let thread = std::thread::spawn(move || writing.run(|| Copied::State(Box::new(state()))));
        std::thread::sleep(Duration::from_millis(100));
        assert!(persister.writes.lock().unwrap().is_empty(), "written while restoring");
        persister.arm();
        let deadline = Instant::now() + Duration::from_secs(5);
        while persister.writes.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "never written once armed");
            std::thread::sleep(Duration::from_millis(10));
        }
        persister.stopping(state());
        thread.join().unwrap();
    }

    #[test]
    fn a_state_the_next_start_would_refuse_is_never_written() {
        let scratch = Scratch::new("invalid");
        let persister = Persister::new(scratch.file(), false);
        let mut invalid = state();
        invalid.tabs[0].zoomed = Some("p9".to_string());
        persister.write_if_changed(&invalid, &mut Vec::new());
        assert_eq!(load(&scratch.file()), Loaded::Nothing, "the last file stays");
    }

    /// Every fixture, and the settings it holds at something other than their defaults. Each
    /// setting is held by exactly one, the first written after it was added.
    const FIXTURES: [(&str, &[&str]); 4] = [
        ("state-v1.json", &["shell", "scrollback_bytes", "palette", "clipboard_write", "cursor"]),
        ("state-v1-scroll-multiplier.json", &["scroll_multiplier"]),
        ("state-v1-name-sessions.json", &["name_sessions"]),
        ("state-v1-human-name.json", &["human_name"]),
    ];

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// The state `tests/fixtures/state-v1.json` holds: every setting other than its default, a
    /// zoomed split each way and labels. Frozen with the file, so it shares nothing with the
    /// other tests' states.
    #[allow(
        clippy::needless_update,
        reason = "a setting added later compiles here, and takes its default as the file does"
    )]
    fn fixture_v1() -> State {
        let grid = Grid { cols: 80, rows: 24, width_px: 800, height_px: 480 };
        let pane = |name: &str, label: Option<&str>| Pane {
            name: name.to_string(),
            label: label.map(str::to_string),
            cwd: PathBuf::from("/tmp"),
            grid,
        };
        let split = |axis, ratio, first: &str, second: &str| Node::Split {
            axis,
            ratio,
            first: Box::new(Node::Pane(first.to_string())),
            second: Box::new(Node::Pane(second.to_string())),
        };
        State {
            version: 1,
            daemon: "0.9.0".to_string(),
            settings: proto::Settings {
                shell: Some(proto::Shell {
                    command: Some("/bin/zsh".to_string()),
                    mode: proto::ShellMode::NonLogin.into(),
                    ..Default::default()
                }),
                scrollback_bytes: Some(5_000_000),
                palette: Some(proto::Palette {
                    entries: vec![0x10_20_30, 0xc0_40_40],
                    foreground: 0xdd_dd_dd,
                    background: 0x11_11_11,
                    cursor: Some(0xff_00_ff),
                    scheme: proto::ColorScheme::Dark.into(),
                    ..Default::default()
                }),
                clipboard_write: Some(false),
                cursor: Some(proto::Cursor {
                    style: proto::CursorStyle::Bar.into(),
                    blink: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
            tabs: vec![
                Tab {
                    name: "t1".to_string(),
                    label: proto::Label { text: Some("work".to_string()), generation: 2 },
                    zoomed: Some("p2".to_string()),
                    root: split(Axis::Columns, 0.3, "p1", "p2"),
                },
                Tab {
                    name: "t2".to_string(),
                    label: proto::Label { text: None, generation: 1 },
                    zoomed: None,
                    root: split(Axis::Rows, 0.6, "p3", "p4"),
                },
            ],
            panes: vec![
                pane("p1", Some("A")),
                pane("p2", None),
                pane("p3", None),
                pane("p4", Some("B")),
            ],
        }
    }

    /// A file an earlier daemon wrote is read as it was meant, whatever has changed since. A
    /// field added since takes its default here as it does in the file.
    #[test]
    fn a_version_1_file_reads_as_it_was_written() {
        assert_eq!(load(&fixture("state-v1.json")), Loaded::State(Box::new(fixture_v1())));
    }

    /// Whatever a fixture says survives reading it and writing it again, under the same names:
    /// a field renamed or retyped since would be missing or different.
    #[test]
    fn every_fixture_reads_back_everything_it_holds() {
        fn within(held: &serde_json::Value, read: &serde_json::Value, at: &str) {
            match (held, read) {
                (serde_json::Value::Object(held), serde_json::Value::Object(read)) => {
                    for (key, value) in held {
                        let path = format!("{at}.{key}");
                        within(value, read.get(key).unwrap_or(&serde_json::Value::Null), &path);
                    }
                }
                (serde_json::Value::Array(held), serde_json::Value::Array(read))
                    if held.len() == read.len() =>
                {
                    for (index, (held, read)) in held.iter().zip(read).enumerate() {
                        within(held, read, &format!("{at}[{index}]"));
                    }
                }
                // A ratio is an f32, which reads back as the nearest f64 to it.
                (serde_json::Value::Number(held), serde_json::Value::Number(read)) => {
                    let (held, read) = (held.as_f64().unwrap(), read.as_f64().unwrap());
                    assert!((held - read).abs() < 1e-6, "{at} reads back as {read}, not {held}");
                }
                _ => assert_eq!(held, read, "{at} reads back differently"),
            }
        }
        for (name, _) in FIXTURES {
            let bytes = std::fs::read(fixture(name)).unwrap();
            let held: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let Loaded::State(state) = load(&fixture(name)) else { panic!("{name} does not load") };
            within(&held, &serde_json::to_value(state).unwrap(), name);
        }
    }

    /// A setting a fixture holds at its default would read back as the default after a rename,
    /// and pass.
    #[test]
    fn every_fixture_holds_its_settings_at_something_other_than_their_defaults() {
        let default = serde_json::to_value(proto::Settings::default()).unwrap();
        for (name, settings) in FIXTURES {
            let Loaded::State(state) = load(&fixture(name)) else { panic!("{name} does not load") };
            let held = serde_json::to_value(state.settings).unwrap();
            for setting in settings {
                assert!(default.get(setting).is_some(), "{setting} is not a setting");
                assert_ne!(
                    held.get(setting),
                    default.get(setting),
                    "{name} leaves {setting} at its default"
                );
            }
        }
    }

    /// A setting added to the protocol is saved, so a restart must keep it, and nothing proves
    /// it does until a fixture holds it. That is a new fixture, never an edit to a frozen one.
    #[test]
    fn every_setting_is_held_by_a_fixture() {
        let default = serde_json::to_value(proto::Settings::default()).unwrap();
        let held: Vec<&str> =
            FIXTURES.iter().flat_map(|(_, settings)| settings.iter().copied()).collect();
        for setting in default.as_object().unwrap().keys() {
            assert!(
                held.contains(&setting.as_str()),
                "no fixture holds {setting}: add one, a copy of the latest with {setting} set, to \
                 FIXTURES"
            );
        }
    }

    #[test]
    fn a_state_this_daemon_could_not_have_written_is_corrupt() {
        let corrupt = |change: fn(&mut State)| {
            let mut state = state();
            change(&mut state);
            validate(&state).expect_err("not a state a daemon writes")
        };
        corrupt(|state| state.tabs[0].name = "a tab".to_string());
        corrupt(|state| state.panes.push(pane("p3")));
        corrupt(|state| {
            state.panes.pop();
        });
        corrupt(|state| state.tabs.push(state.tabs[0].clone()));
        corrupt(|state| state.tabs[0].zoomed = Some("p9".to_string()));
        corrupt(|state| {
            if let Node::Split { ratio, .. } = &mut state.tabs[0].root {
                *ratio = 1.5;
            }
        });
        corrupt(|state| state.panes[0].grid.cols = 0);
        corrupt(|state| {
            state.tabs[0].root = Node::Split {
                axis: Axis::Rows,
                ratio: 0.5,
                first: Box::new(Node::Pane("p1".to_string())),
                second: Box::new(Node::Pane("p1".to_string())),
            };
        });
    }
}
