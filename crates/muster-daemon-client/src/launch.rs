//! Starting this machine's daemon, or adopting the one already running, which is
//! `muster-daemon-launch`'s, and stopping one, which needs a control connection.

use std::path::Path;
use std::time::{Duration, Instant};

use muster_daemon_proto::ConnectionKind;

pub use muster_daemon_launch::launch::*;

/// How often a stopping daemon is dialled.
const DIAL_INTERVAL: Duration = Duration::from_millis(2);

/// Ends the daemon on `socket` and every pane in it, and waits up to `patience` for it to go.
pub fn stop(socket: &Path, patience: Duration) -> Result<(), String> {
    let control =
        crate::control::Control::open(socket, "muster stop", |_, _| {}).map_err(|error| {
            format!(
                "could not reach the daemon on {} to stop it ({error}), so nothing was stopped. \
                 If no daemon runs there, there is nothing to stop; if one does, its log beside \
                 the socket says why it would not talk.",
                socket.display()
            )
        })?;
    control.stop().wait(patience).map_err(|why| {
        format!(
            "the daemon on {} was asked to stop and {why}, so it may still be running with its \
             panes, or be part way through closing them. Dial it again to see whether it \
             answers; its log beside the socket says what it did with the request.",
            socket.display()
        )
    })?;
    let deadline = Instant::now() + patience;
    while answers(socket) {
        if Instant::now() >= deadline {
            return Err(format!(
                "the daemon on {} agreed to stop and is still answering after {patience:?}; its \
                 panes may still be closing",
                socket.display()
            ));
        }
        std::thread::sleep(DIAL_INTERVAL);
    }
    Ok(())
}

fn answers(socket: &Path) -> bool {
    crate::dial(socket, ConnectionKind::Control, "muster probe").is_ok()
}
