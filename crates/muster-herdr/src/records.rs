//! The daemons Muster started, written down and checked back.
//!
//! `muster_core::daemons` owns what a record says; this owns the directory it lives in and the
//! dial that turns a record into an answer. The same division `names.rs` and `shared_names.rs`
//! draw, and for the same reason: what a file means is portable and where a file is is not.
//!
//! **One file per daemon, keyed by socket, so nothing needs a lock.** Two windows starting
//! daemons start them on different sockets - a second window on the same socket adopts rather
//! than starts - so no two writers ever reach for one file. That is what lets this be a plain
//! write where the shared name registry needs a hold.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use muster_core::daemons::{Started, beyond_the_bound, from_toml, holding, to_toml};
use muster_core::diagnostics::log;
use muster_core::fields;
use serde_json::{Value, json};

use crate::client::HerdrClient;
use crate::daemon::answers;

/// What a daemon named in a record turned out to be doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Census {
    pub socket: String,
    pub started: u64,
    pub state: State,
    /// How many panes it holds and where, for a daemon that answered.
    pub panes: u32,
    pub directories: Vec<String>,
}

/// Whether the daemon a record names is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Dialed, and it replied.
    Answering,
    /// Its socket file is there and nothing answers on it - a daemon that exited without
    /// tidying up after itself.
    Silent,
    /// No socket file left at all, which is the one case that cannot be resolved from here: a
    /// daemon whose socket path was deleted out from under it is still running and unreachable,
    /// and looks exactly like one that ended.
    Gone,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Answering => "answering",
            State::Silent => "silent",
            State::Gone => "gone",
        }
    }
}

/// Writes down a daemon Muster has just started.
///
/// Called only for a daemon Muster started, never for one it adopted. Muster can vouch for what
/// it started; a daemon that was already answering belongs to whoever started it, and claiming
/// it in this census would offer somebody a process to end on Muster's word.
///
/// Failures are logged and swallowed. A window whose daemon started is a working window, and
/// refusing to open it because a record could not be written would trade the feature for the
/// thing the feature is about.
pub fn started(directory: &str, socket: &str) {
    let directory = Path::new(directory);
    if let Err(error) = std::fs::create_dir_all(directory) {
        return complain("daemons.record.unwritable", socket, &error.to_string());
    }

    let existing = read_directory(directory);
    let record = Started { socket: socket.to_string(), started: now() };
    // A daemon restarted on the same socket replaces its own row rather than adding one.
    let path = if let Some(path) = holding(&existing, socket) {
        path.clone()
    } else {
        for stale in beyond_the_bound(&existing) {
            let _ = std::fs::remove_file(&stale);
        }
        mint(directory, &existing)
    };
    if let Err(error) = std::fs::write(&path, to_toml(&record)) {
        complain("daemons.record.unwritable", socket, &error.to_string());
    }
}

/// Every daemon in the record, with what it is doing now.
///
/// The dial is the point. A record says a daemon was started on a socket, and only asking says
/// whether one is there - so nothing here reports liveness from the file, and a record that
/// cannot be checked reads as [`State::Gone`] rather than as an absence.
///
/// Newest first, which is the order somebody scanning for the daemon they just started wants.
/// Not an ordering anything should act on: age picked the wrong process on the machine that
/// prompted this.
pub fn census(directory: &str) -> Vec<Census> {
    let mut found: Vec<Census> =
        read_directory(Path::new(directory)).into_iter().map(|(_, record)| look(record)).collect();
    found.sort_by(|left, right| {
        right.started.cmp(&left.started).then_with(|| left.socket.cmp(&right.socket))
    });
    found
}

/// One record, checked.
fn look(record: Started) -> Census {
    let state = if !Path::new(&record.socket).exists() {
        State::Gone
    } else if answers(&record.socket) {
        State::Answering
    } else {
        State::Silent
    };
    let (panes, directories) =
        if state == State::Answering { held_by(&record.socket) } else { (0, Vec::new()) };
    Census { socket: record.socket, started: record.started, state, panes, directories }
}

/// What one answering daemon holds, asked of it rather than guessed from its path.
///
/// The same question `./dev --doctor` asks with `herdr pane list`, over the API rather than
/// over a subprocess - which is what makes it something a person running Muster has rather than
/// something this repo's build script has.
fn held_by(socket: &str) -> (u32, Vec<String>) {
    let Ok(answer) = HerdrClient::new(socket.to_string()).request("pane.list", &json!({})) else {
        return (0, Vec::new());
    };
    let panes = answer
        .as_object()
        .and_then(|body| body.values().find_map(|value| value.get("panes")))
        .or_else(|| answer.get("panes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut directories: Vec<String> = panes
        .iter()
        .filter_map(|pane| pane.get("cwd").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    directories.dedup();
    (u32::try_from(panes.len()).unwrap_or(u32::MAX), directories)
}

/// Every readable record in the directory, paired with the file it came from.
fn read_directory(directory: &Path) -> Vec<(PathBuf, Started)> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<(PathBuf, Started)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .filter_map(|path| {
            let record = std::fs::read_to_string(&path).ok()?;
            from_toml(&record).map(|started| (path, started))
        })
        .collect();
    // Settled, so two runs of a census over an unchanged directory read the same - a directory
    // hands its entries back in whatever order it likes.
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found
}

/// A file name nothing is using, numbered the way the saved arrangements are.
///
/// Numbered rather than derived from the socket, which is the obvious alternative and needs an
/// encoding: a socket path is not a file name, and two paths that differ only in a separator
/// would collide under any cheap flattening of one. The socket lives inside the file, which is
/// where anything looking for it reads it.
fn mint(directory: &Path, existing: &[(PathBuf, Started)]) -> PathBuf {
    let taken: Vec<&PathBuf> = existing.iter().map(|(path, _)| path).collect();
    let mut number = 1;
    loop {
        let candidate = directory.join(format!("daemon-{number}.toml"));
        if !taken.contains(&&candidate) && !candidate.exists() {
            return candidate;
        }
        number += 1;
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or_default()
}

fn complain(event: &str, socket: &str, detail: &str) {
    log::warn(
        event,
        fields! {
            "socket" => socket,
            "detail" => detail,
            "impact" => "this daemon is missing from `muster daemons`, so somebody deciding \
                         which herdr process to end is not shown one Muster started - and it is \
                         the census that makes ending one safe",
            "check" => "whether ~/.muster/state/daemons is writable; the window itself is \
                        unaffected and its panes work normally",
        },
    );
}
