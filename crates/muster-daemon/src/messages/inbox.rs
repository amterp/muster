//! Claude Code's inbox socket, as a wake adapter (MIP-4, section 6): one line carrying the
//! notice, on a connection opened only once the line is ready, since Claude Code closes one that
//! sends nothing for 30 seconds. No auth line: from a process that is not the session's child
//! the token changes nothing (`docs/observations/claude-code-2.1.283.md`), so the daemon never
//! holds it.

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use muster_msg::{Inbox, Participant, Presence};

/// How long a write to an inbox may take. A session's inbox is served by its own event loop on
/// this machine, so a line that takes longer is one that is not being read.
const PATIENCE: Duration = Duration::from_secs(1);

/// Whether the socket at the inbox's path is the one the participant joined with: a session
/// that has gone leaves its path to be reused by the next process with its id.
fn is_same_socket(inbox: &Inbox) -> bool {
    std::fs::metadata(Path::new(&inbox.socket)).is_ok_and(|metadata| metadata.ino() == inbox.inode)
}

/// Hands the session one message, returning why it could not.
pub(crate) fn deliver(inbox: &Inbox, text: &str) -> Result<(), String> {
    if !is_same_socket(inbox) {
        return Err(format!("{} is gone, or belongs to another session now", inbox.socket));
    }
    let mut connection =
        UnixStream::connect(&inbox.socket).map_err(|error| format!("{}: {error}", inbox.socket))?;
    connection.set_write_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    let message = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    let mut line = message.to_string();
    line.push('\n');
    connection.write_all(line.as_bytes()).map_err(|error| format!("{}: {error}", inbox.socket))
}

/// Whether a participant is still there: a session's inbox still accepts a connection. A
/// connection that sends nothing shows nothing in the session (the observation above, section
/// 3). A participant with no inbox can never be woken, so it is not there: it keeps its groups
/// and its place in them, and its name is free to be taken over.
#[derive(Debug)]
pub(crate) struct Sockets;

impl Presence for Sockets {
    fn alive(&self, participant: &Participant) -> bool {
        participant.inbox.as_ref().is_some_and(|inbox| {
            is_same_socket(inbox) && UnixStream::connect(&inbox.socket).is_ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A participant made by `join --name` from a plain shell, or left behind when a session
    /// joined under another name, can never be woken; counting it alive kept its name from
    /// every later session.
    #[test]
    fn a_participant_nothing_can_reach_is_not_alive() {
        let participant = Participant {
            name: "critic".to_string(),
            inbox: None,
            pane: None,
            gone: false,
            cursors: std::collections::BTreeMap::default(),
            woken: std::collections::BTreeSet::default(),
            rewoken: std::collections::BTreeSet::default(),
            pull: false,
        };
        assert!(!Sockets.alive(&participant));
    }
}
