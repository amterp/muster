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
//!   `#[serde(alias)]` naming the old one, added in `muster-daemon-proto/build.rs`. Enums are
//!   stored as numbers, which a renamed enum value leaves alone.
//! - Anything else bumps [`VERSION`], and [`load`] goes on reading every earlier version.
//! - A fixture is frozen once written. A new version gets a fixture of its own.

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
    State(State),
    /// Written by a newer daemon, in a format this one cannot read.
    Newer(u32),
    /// Not a state this daemon could have written.
    Corrupt(String),
    /// There, and not readable.
    Unreadable(String),
}

pub(crate) fn load(path: &Path) -> Loaded {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::Nothing,
        Err(error) => return Loaded::Unreadable(error.to_string()),
    };
    // The version first, on its own: a newer daemon's file may not parse as this one's at all,
    // and must still be told apart from a damaged one.
    let version = match serde_json::from_slice::<Versioned>(&bytes) {
        Ok(versioned) => versioned.version,
        Err(error) => return Loaded::Corrupt(error.to_string()),
    };
    if version > VERSION {
        return Loaded::Newer(version);
    }
    match serde_json::from_slice::<State>(&bytes) {
        Ok(state) => match validate(&state) {
            Ok(()) => Loaded::State(state),
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
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".corrupt-{seconds}"));
    let aside = PathBuf::from(aside);
    std::fs::rename(path, &aside)?;
    Ok(aside)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Saved tabs are coming back. Nothing is written: until they are all back the session
    /// holds less than the file, and a write would lose the difference.
    Restoring,
    Writing,
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
            pending: Mutex::new(Pending { phase, changed: None, last: None }),
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

    /// The daemon is stopping, and this is the state to leave behind: what it held before it
    /// closed anything. Only the first call counts.
    pub(crate) fn stopping(&self, state: State) {
        let mut pending = self.pending();
        match pending.phase {
            Phase::Writing => {
                pending.phase = Phase::Stopping;
                pending.last = Some(state);
            }
            Phase::Restoring => pending.phase = Phase::Stopped,
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
                Phase::Restoring => {
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
            drop(pending);
            match copy() {
                Copied::State(state) => self.write_if_changed(&state, &mut written),
                // What the daemon held before it began to stop is the state to leave, and it
                // was handed over in the same hold of the session's lock that this copy saw.
                Copied::Stopping => {}
                Copied::Gone => return,
            }
            pending = self.pending();
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
        assert_eq!(load(&scratch.file()), Loaded::State(state()));
    }

    #[test]
    fn a_write_cut_off_before_its_rename_leaves_the_last_state_readable() {
        let scratch = Scratch::new("cut-off");
        written(&scratch, &state());
        write_temporary(&scratch.file(), b"{\"version\": 1, \"tabs\": [{\"tab\"").unwrap();
        assert_eq!(load(&scratch.file()), Loaded::State(state()));
        let mut next = state();
        next.tabs[0].label.generation = 3;
        written(&scratch, &next);
        assert_eq!(load(&scratch.file()), Loaded::State(next), "the leftover is written over");
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
    fn a_state_the_next_start_would_refuse_is_never_written() {
        let scratch = Scratch::new("invalid");
        let persister = Persister::new(scratch.file(), false);
        let mut invalid = state();
        invalid.tabs[0].zoomed = Some("p9".to_string());
        persister.write_if_changed(&invalid, &mut Vec::new());
        assert_eq!(load(&scratch.file()), Loaded::Nothing, "the last file stays");
    }

    /// The state `tests/fixtures/state-v1.json` holds: every setting other than its default, a
    /// zoomed split and labels.
    fn fixture_v1() -> State {
        State {
            version: 1,
            daemon: "0.9.0".to_string(),
            settings: proto::Settings {
                shell: Some(proto::Shell {
                    command: Some("/bin/zsh".to_string()),
                    mode: proto::ShellMode::NonLogin.into(),
                }),
                scrollback_bytes: Some(5_000_000),
                palette: Some(proto::Palette {
                    entries: vec![0x10_20_30, 0xc0_40_40],
                    foreground: 0xdd_dd_dd,
                    background: 0x11_11_11,
                    cursor: Some(0xff_00_ff),
                    scheme: proto::ColorScheme::Dark.into(),
                }),
                clipboard_write: Some(false),
                cursor: Some(proto::Cursor {
                    style: proto::CursorStyle::Bar.into(),
                    blink: Some(false),
                }),
            },
            ..state()
        }
    }

    /// A file an earlier daemon wrote is read as it was meant, whatever has changed since. The
    /// file is frozen: a later format gets a fixture of its own, never an edit to this one.
    #[test]
    fn a_version_1_file_reads_as_it_was_written() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/state-v1.json");
        assert_eq!(load(&path), Loaded::State(fixture_v1()));
    }

    /// Every setting the fixture holds is one a restart must keep, so each must differ from its
    /// default, or a renamed field would read back as the default and pass. A setting added
    /// later goes in a later version's fixture.
    #[test]
    fn the_version_1_fixture_sets_every_setting_it_has() {
        let set = serde_json::to_value(fixture_v1().settings).unwrap();
        let default = serde_json::to_value(proto::Settings::default()).unwrap();
        let (set, default) = (set.as_object().unwrap(), default.as_object().unwrap());
        for (name, value) in default {
            assert_ne!(set.get(name), Some(value), "{name} is its default in the fixture");
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
