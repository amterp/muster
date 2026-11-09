//! The daemons Muster started, checked back: the census `muster daemons` answers from.
//!
//! `muster_core::daemons` owns what a record says, `muster-daemon-launch` writes one when a
//! daemon is started, and this owns the dial that turns a record into an answer. The same
//! division `names.rs` and the seam's `holding.rs` draw, and for the same reason: what a file
//! means is portable and where a file is is not.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use muster_core::daemons::Dial;
use muster_daemon_launch::records::read_directory;
use muster_daemon_proto::{ConnectionKind, answer};

pub use muster_core::daemons::{Census, Holding, State};
pub use muster_daemon_launch::records::started;

use crate::control::Control;

/// How long a daemon that answered its handshake gets to say what it holds.
const SNAPSHOT_PATIENCE: Duration = Duration::from_secs(5);

/// Every daemon in the record, with what it is doing now: `muster_core::daemons::census`, dialing
/// each socket on a connection of this client's.
pub fn census(directory: &str) -> Vec<Census> {
    let records = read_directory(Path::new(directory)).into_iter().map(|(_, record)| record);
    muster_core::daemons::census(records, &Dialed)
}

struct Dialed;

impl Dial for Dialed {
    fn exists(&self, socket: &str) -> bool {
        Path::new(socket).exists()
    }

    fn listening(&self, socket: &str) -> bool {
        UnixStream::connect(socket).is_ok()
    }

    fn welcome(&self, socket: &str) -> Option<String> {
        crate::dial(Path::new(socket), ConnectionKind::Control, "muster census")
            .ok()
            .map(|(_, welcome)| welcome.daemon_version)
    }

    fn holding(&self, socket: &str) -> Result<(u32, Vec<String>), String> {
        held_by(socket)
    }
}

/// What one answering daemon holds, asked of it rather than guessed from its path, or why it
/// could not be asked. A daemon that answered its handshake and then would not say is not one
/// that holds nothing.
fn held_by(socket: &str) -> Result<(u32, Vec<String>), String> {
    let control = Control::open(Path::new(socket), "muster census", |_, _| {})
        .map_err(|error| format!("a second connection to ask what it holds failed: {error}"))?;
    let answer = control
        .snapshot()
        .wait(SNAPSHOT_PATIENCE)
        .map_err(|error| format!("asking what it holds failed: {error}"))?;
    let Some(answer::Detail::Snapshot(snapshot)) = answer.detail else {
        return Err(format!(
            "it answered a request for what it holds with something else ({})",
            answer.reason
        ));
    };
    let mut directories: Vec<String> = snapshot.panes.iter().map(|pane| pane.cwd.clone()).collect();
    directories.dedup();
    Ok((u32::try_from(snapshot.panes.len()).unwrap_or(u32::MAX), directories))
}
