//! When the daemon saw a pane's agent enter its current state (`Pane.state_since_ms`).
//!
//! The state dated is detection's reading with stuck laid over it (`proto::Pane::state`), which a
//! window also counts from, so a window and the CLI agree on how long an agent has been in it.
//! Unknown is a real answer: an agent a restart resumed was already in its state before this
//! daemon could see it, and a time filled in here would read as the moment it began.

use std::time::{SystemTime, UNIX_EPOCH};

use muster_daemon_proto as proto;

/// Dates the state `record` is in now, which was `before`.
///
/// A pane whose time is unknown and that had no state to show is one whose agent this daemon is
/// meeting for the first time without having started it, as after a restart resumed it, so the
/// state it is first read in stays undated.
pub(crate) fn dated(before: proto::PaneState, record: &mut proto::Pane, now_ms: u64) {
    if record.state() == before {
        return;
    }
    if record.state_since_ms.is_none() && before == proto::PaneState::Unknown {
        return;
    }
    record.state_since_ms = Some(now_ms);
}

/// Milliseconds since the Unix epoch, by this daemon's clock.
pub(crate) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::AgentState::{Idle, Unknown, Working};

    fn pane(state: proto::AgentState, since: Option<u64>) -> proto::Pane {
        let mut record = proto::Pane {
            agent: (state != Unknown).then(|| "claude".to_string()),
            state_since_ms: since,
            ..proto::Pane::default()
        };
        record.set_agent_state(state);
        record
    }

    fn stuck(mut record: proto::Pane) -> proto::Pane {
        record.unread_input = Some(proto::UnreadInput::default());
        record
    }

    /// `record` once it has gone from `before` to the state it holds, at time 20.
    fn after(before: &proto::Pane, mut record: proto::Pane) -> Option<u64> {
        record.state_since_ms = before.state_since_ms;
        dated(before.state(), &mut record, 20);
        record.state_since_ms
    }

    #[test]
    fn a_change_is_dated_and_anything_else_keeps_the_date() {
        assert_eq!(after(&pane(Idle, Some(10)), pane(Working, None)), Some(20));
        let titled = proto::Pane { title: "busy".to_string(), ..pane(Idle, None) };
        assert_eq!(after(&pane(Idle, Some(10)), titled), Some(10), "a title is not a state");
        assert_eq!(after(&pane(Working, Some(10)), pane(Unknown, None)), Some(20), "it left");
    }

    #[test]
    fn an_agent_met_without_a_date_stays_undated_until_it_changes() {
        assert_eq!(after(&pane(Unknown, None), pane(Idle, None)), None, "met already idle");
        assert_eq!(after(&pane(Idle, None), pane(Working, None)), Some(20), "then seen to work");
        assert_eq!(after(&pane(Unknown, Some(10)), pane(Idle, None)), Some(20), "seen to arrive");
    }

    #[test]
    fn stuck_is_a_state_of_its_own_whatever_was_read_under_it() {
        assert_eq!(after(&pane(Working, Some(10)), stuck(pane(Working, None))), Some(20));
        assert_eq!(after(&stuck(pane(Working, Some(10))), stuck(pane(Idle, None))), Some(10));
        assert_eq!(after(&stuck(pane(Idle, Some(10))), pane(Idle, None)), Some(20), "reading");
    }
}
