//! The daemons Muster started, checked back: the census `muster daemons` answers from.
//!
//! `muster_core::daemons` owns what a record says, `muster-daemon-launch` writes one when a
//! daemon is started, and this owns the dial that turns a record into an answer. The same
//! division `names.rs` and the seam's `holding.rs` draw, and for the same reason: what a file
//! means is portable and where a file is is not.

use std::os::unix::net::UnixStream;
use std::path::Path;

use muster_core::daemons::Started;
use muster_daemon_launch::records::read_directory;
use muster_daemon_proto::{ConnectionKind, answer};

pub use muster_daemon_launch::records::started;

use crate::control::Control;

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
    /// A herdr daemon, which a Muster from before muster-daemon started and wrote down here.
    /// Something still listens on its socket, so it is running, keeping its panes alive, and
    /// no window of this Muster shows them.
    Herdr,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Answering => "answering",
            State::Silent => "silent",
            State::Gone => "gone",
            State::Herdr => "herdr",
        }
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
    } else if is_herdr(&record.socket) {
        // Never sent muster-daemon's handshake, which herdr would not answer: a listener is
        // all there is to learn from it.
        if UnixStream::connect(&record.socket).is_ok() { State::Herdr } else { State::Silent }
    } else if answers(&record.socket) {
        State::Answering
    } else {
        State::Silent
    };
    let (panes, directories) =
        if state == State::Answering { held_by(&record.socket) } else { (0, Vec::new()) };
    Census { socket: record.socket, started: record.started, state, panes, directories }
}

/// Whether `socket` is where a Muster from before muster-daemon had herdr listen, which was
/// always a file of this name.
fn is_herdr(socket: &str) -> bool {
    Path::new(socket).file_name().is_some_and(|name| name == "herdr.sock")
}

/// Whether a daemon answers on `socket`, by its handshake.
fn answers(socket: &str) -> bool {
    crate::dial(Path::new(socket), ConnectionKind::Control, "muster census").is_ok()
}

/// What one answering daemon holds, asked of it rather than guessed from its path.
fn held_by(socket: &str) -> (u32, Vec<String>) {
    let Ok(control) = Control::open(Path::new(socket), "muster census", |_, _| {}) else {
        return (0, Vec::new());
    };
    let Ok(answer) = control.snapshot().wait(std::time::Duration::from_secs(5)) else {
        return (0, Vec::new());
    };
    let Some(answer::Detail::Snapshot(snapshot)) = answer.detail else { return (0, Vec::new()) };
    let mut directories: Vec<String> = snapshot.panes.iter().map(|pane| pane.cwd.clone()).collect();
    directories.dedup();
    (u32::try_from(snapshot.panes.len()).unwrap_or(u32::MAX), directories)
}
