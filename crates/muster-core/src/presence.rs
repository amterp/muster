//! The person at the desk, as the app tells the daemons it attends (MIP-10, section 4): whether
//! Muster is in front, and when they last used it.
//!
//! A report stands at the daemon for [`LASTS`], and the app renews it every [`HEARTBEAT`], so a
//! report one heartbeat late on a loaded machine never reads as the person gone, while an app that
//! hangs reads as unknown within a minute. One that quits or crashes closes its connection, which
//! ends its report at once. Between heartbeats a report goes at once only when it says something a
//! client would act on soon: Muster coming forward or going behind, or somebody using it after the
//! daemon last heard of no use for a heartbeat or more. Typing in between is carried by the next
//! heartbeat, so a keystroke costs a clock read and nothing on the wire.
//!
//! Pure: no clock and no socket. The caller says when, and sends what this decides.

use std::time::Duration;

/// How often the app renews its report.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long each report stands at the daemon: three heartbeats.
pub const LASTS: Duration = Duration::from_secs(45);

/// What a report says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub in_front: bool,
    /// When the person last used Muster, in UTC milliseconds; none since it started.
    pub last_input_ms: Option<u64>,
}

/// What the app knows of the person, and what it last told the daemons.
#[derive(Debug, Default)]
pub struct Presence {
    now: Report,
    /// The use the daemons last heard of, which decides whether a new one is news.
    reported_input_ms: Option<u64>,
}

impl Presence {
    /// Muster came forward or went behind. Whether to report at once: when that changed.
    pub fn in_front(&mut self, in_front: bool) -> bool {
        let changed = self.now.in_front != in_front;
        self.now.in_front = in_front;
        changed
    }

    /// The person used Muster at `now_ms`. Whether to report at once: when the daemons last
    /// heard of no use for a heartbeat or more. Otherwise the next heartbeat carries it.
    pub fn used(&mut self, now_ms: u64) -> bool {
        self.now.last_input_ms = Some(now_ms);
        let heartbeat_ms = u64::try_from(HEARTBEAT.as_millis()).unwrap_or(u64::MAX);
        self.reported_input_ms
            .is_none_or(|reported| now_ms.saturating_sub(reported) >= heartbeat_ms)
    }

    /// The report to send now, which the daemons are then taken to have heard.
    pub fn report(&mut self) -> Report {
        self.reported_input_ms = self.now.last_input_ms;
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coming_forward_and_going_behind_are_news_once_each() {
        let mut presence = Presence::default();
        assert!(presence.in_front(true));
        assert!(!presence.in_front(true));
        assert!(presence.report().in_front);
        assert!(presence.in_front(false));
    }

    #[test]
    fn use_after_a_heartbeat_without_any_is_news_and_typing_on_is_not() {
        let mut presence = Presence::default();
        assert!(presence.used(100_000), "the first use is news");
        assert_eq!(presence.report().last_input_ms, Some(100_000));
        assert!(!presence.used(101_000), "typing on waits for the heartbeat");
        assert_eq!(presence.report().last_input_ms, Some(101_000), "the heartbeat carries it");
        assert!(!presence.used(110_000));
        assert!(presence.used(101_000 + 15_000), "back after a heartbeat with none is news");
        assert_eq!(presence.report().last_input_ms, Some(116_000));
    }
}
