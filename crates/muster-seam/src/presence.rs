//! The person at the desk, as this app tells the daemons it attends (MIP-10, section 4): what
//! `muster_core::presence` decides, fed from the requests the shell sends and read by each
//! attended daemon's follower at every connect and heartbeat.
//!
//! A lock of its own rather than the session's, since every keystroke takes it. Never held while
//! the session's is taken: a report sent at once walks the daemons under the session's lock, and
//! a follower reads this one to send it.

use std::sync::{LazyLock, Mutex};

use muster_core::diagnostics::{clock, poison};
use muster_core::presence::Presence;
use muster_daemon_client::follow::PresenceSource;

use crate::proto::request;
use crate::session;

static PRESENCE: LazyLock<Mutex<Presence>> = LazyLock::new(|| Mutex::new(Presence::default()));

/// What each attended daemon's follower reports.
pub(crate) fn source() -> PresenceSource {
    PresenceSource::new(|| poison::lock(&PRESENCE, "presence").report())
}

/// Whether a request from the shell is the person using Muster: typing, pasting, scrolling or
/// clicking in a window. Not a request through the command socket, which is a script's or an
/// agent's.
pub(crate) fn is_use(payload: &request::Payload) -> bool {
    matches!(
        payload,
        request::Payload::KeyDown(_)
            | request::Payload::SendText(_)
            | request::Payload::Paste(_)
            | request::Payload::Wheel(_)
            | request::Payload::Mouse(_)
    )
}

/// The person used Muster just now.
pub(crate) fn used() {
    let now_ms = u64::try_from(clock::wall_clock_millis()).unwrap_or_default();
    let now = poison::lock(&PRESENCE, "presence").used(now_ms);
    if now {
        session::report_presence_now();
    }
}

/// Muster came forward or went behind.
pub(crate) fn in_front(in_front: bool) {
    let now = poison::lock(&PRESENCE, "presence").in_front(in_front);
    if now {
        session::report_presence_now();
    }
}
