//! What waits on the human, kept whether or not a window is open, and the human's presence at the
//! desk as the windows attending this daemon report it (MIP-10, section 4).
//!
//! Messages for the human are the other half of what waits on them, and are not here: the message
//! service decides what waits in each group, and the session keeps it (`Session::human`). Which
//! states ask, and when a quiet helper asks after all, are the rules a window applies too
//! (`muster_core::attention`, `muster_core::helpers`); what is here is this daemon's reading of its
//! own panes by them, kept as state so that a change is an event.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use muster_core::AgentState;
use muster_core::attention::Alert;
use muster_core::helpers::{MakerHearing, quiet_in_effect};
use muster_daemon_proto as proto;

use crate::session::Shared;

/// How long a presence report stands when it does not say.
pub(crate) const REPORT_LASTS: Duration = Duration::from_secs(45);

/// The longest a presence report may stand: a report that claimed longer would keep a crashed
/// app reading as present.
const LONGEST_REPORT: Duration = Duration::from_mins(10);

/// How long the doorbell's thread may sleep for all presence cares, with no report standing.
const NOTHING_DUE: Duration = Duration::from_mins(1);

/// What `record`'s pane asks of the human now, given whether its maker can hear it.
pub(crate) fn asking(record: &proto::Pane, maker: MakerHearing) -> Option<proto::Asking> {
    let state = AgentState::of_pane(
        detected(record.agent_state()),
        record.agent.is_some(),
        record.unread_input.is_some(),
    );
    let ask = match Alert::asked_by(state)? {
        Alert::Blocked => proto::Ask::Blocked,
        Alert::Stuck => proto::Ask::Stuck,
        Alert::Message | Alert::Notified | Alert::Done => return None,
    };
    if quiet_in_effect(record.quiet, maker) {
        return None;
    }
    Some(proto::Asking {
        pane: record.pane.clone(),
        ask: ask.into(),
        // Quiet and asking anyway is a helper whose maker cannot hear it.
        unheard_helper: record.quiet,
    })
}

fn detected(state: proto::AgentState) -> AgentState {
    match state {
        proto::AgentState::Working => AgentState::Working,
        proto::AgentState::Blocked => AgentState::Blocked,
        proto::AgentState::Idle => AgentState::Idle,
        proto::AgentState::Unknown => AgentState::Unknown,
    }
}

/// What the daemon keeps of the human's attention.
#[derive(Debug, Default)]
pub(crate) struct Attention {
    /// Every pane asking for the human now, by the pane.
    asking: BTreeMap<String, proto::Asking>,
    /// Each attending connection's standing report, by the connection.
    reports: HashMap<u64, Report>,
    /// Presence as the subscribers were last told it.
    told: proto::Presence,
}

#[derive(Debug, Clone, Copy)]
struct Report {
    in_front: bool,
    last_input_ms: Option<u64>,
    until: Instant,
}

impl Attention {
    /// Every pane asking for the human now.
    pub(crate) fn asking(&self) -> Vec<proto::Asking> {
        self.asking.values().cloned().collect()
    }

    /// Takes what `pane` asks now, nothing when it asks nothing, and returns the event saying so
    /// when that changed: the pane with no ask when it stopped.
    pub(crate) fn ask(&mut self, pane: &str, now: Option<proto::Asking>) -> Option<proto::Asking> {
        match now {
            Some(asking) if self.asking.get(pane) == Some(&asking) => None,
            Some(asking) => {
                self.asking.insert(pane.to_string(), asking.clone());
                Some(asking)
            }
            None => self
                .asking
                .remove(pane)
                .map(|_| proto::Asking { pane: pane.to_string(), ..proto::Asking::default() }),
        }
    }

    /// Takes `connection`'s report, which replaces any it made before.
    pub(crate) fn report(&mut self, connection: u64, report: &proto::ReportPresence, now: Instant) {
        let lasts = match report.lasts_ms {
            0 => REPORT_LASTS,
            said => Duration::from_millis(said).min(LONGEST_REPORT),
        };
        self.reports.insert(
            connection,
            Report {
                in_front: report.in_front,
                last_input_ms: report.last_input_ms,
                until: now + lasts,
            },
        );
    }

    /// Lets go of what `connection` reported, which closed.
    pub(crate) fn left(&mut self, connection: u64) {
        self.reports.remove(&connection);
    }

    /// Lets go of the reports that have run out by `now`, and says when the next one will.
    pub(crate) fn expire(&mut self, now: Instant) -> Option<Instant> {
        self.reports.retain(|_, report| report.until > now);
        self.reports.values().map(|report| report.until).min()
    }

    /// The human's presence as it stands at `now`, with `attended` saying whether a window
    /// attends. Two windows' reports read as one person: in front if either is, and used when
    /// either was last used.
    pub(crate) fn presence(&self, attended: bool, now: Instant) -> proto::Presence {
        let standing: Vec<&Report> =
            self.reports.values().filter(|report| report.until > now).collect();
        proto::Presence {
            attended,
            known: !standing.is_empty(),
            in_front: standing.iter().any(|report| report.in_front),
            last_input_ms: standing.iter().filter_map(|report| report.last_input_ms).max(),
        }
    }

    /// The presence to tell the subscribers, when it differs from what they were last told.
    pub(crate) fn presence_changed(
        &mut self,
        attended: bool,
        now: Instant,
    ) -> Option<proto::Presence> {
        let presence = self.presence(attended, now);
        if presence == self.told {
            return None;
        }
        self.told = presence;
        Some(presence)
    }
}

/// Lets go of presence reports that have run out, telling the subscribers, and says how long the
/// doorbell's thread may sleep before the next one does. On that thread because it is the
/// daemon's one clock that every change already wakes.
pub(crate) fn look(shared: &Shared) -> Duration {
    let now = Instant::now();
    let next = shared.lock().expire_presence(now);
    next.map_or(NOTHING_DUE, |until| until.saturating_duration_since(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(pane: &str) -> proto::Pane {
        let mut record = proto::Pane {
            pane: pane.to_string(),
            agent: Some("claude".to_string()),
            ..proto::Pane::default()
        };
        record.set_agent_state(proto::AgentState::Blocked);
        record
    }

    #[test]
    fn a_blocked_or_stuck_agent_asks_and_a_working_one_does_not() {
        let asked = asking(&blocked("p1"), MakerHearing::None).expect("blocked asks");
        assert_eq!((asked.ask(), asked.unheard_helper), (proto::Ask::Blocked, false));

        let mut stuck = blocked("p1");
        stuck.set_agent_state(proto::AgentState::Working);
        stuck.unread_input = Some(proto::UnreadInput::default());
        assert_eq!(asking(&stuck, MakerHearing::None).map(|a| a.ask()), Some(proto::Ask::Stuck));

        let mut working = blocked("p1");
        working.set_agent_state(proto::AgentState::Working);
        assert_eq!(asking(&working, MakerHearing::None), None);
    }

    #[test]
    fn a_quiet_pane_asks_only_as_a_helper_its_maker_cannot_hear() {
        let quiet = proto::Pane { quiet: true, ..blocked("p2") };
        assert_eq!(asking(&quiet, MakerHearing::None), None);
        assert_eq!(asking(&quiet, MakerHearing::Hears), None);
        let unheard = asking(&quiet, MakerHearing::CannotHear).expect("an unheard helper asks");
        assert!(unheard.unheard_helper);
    }

    #[test]
    fn a_change_in_what_a_pane_asks_is_news_once_and_its_end_says_so() {
        let mut attention = Attention::default();
        let asked = asking(&blocked("p1"), MakerHearing::None);
        assert_eq!(attention.ask("p1", asked.clone()), asked);
        assert_eq!(attention.ask("p1", asked), None, "the same ask again is not news");
        let ended = attention.ask("p1", None).expect("the end is news");
        assert_eq!((ended.pane.as_str(), ended.ask()), ("p1", proto::Ask::Unspecified));
        assert_eq!(attention.ask("p1", None), None);
    }

    #[test]
    fn a_report_stands_until_it_runs_out_or_its_connection_goes() {
        let mut attention = Attention::default();
        let now = Instant::now();
        let report =
            proto::ReportPresence { in_front: true, last_input_ms: Some(7), lasts_ms: 1_000 };
        attention.report(1, &report, now);
        let present = attention.presence(true, now);
        assert_eq!((present.known, present.in_front, present.last_input_ms), (true, true, Some(7)));

        let later = now + Duration::from_millis(1_500);
        assert!(!attention.presence(true, later).known, "a report that ran out stands");
        assert_eq!(attention.expire(later), None);

        attention.report(1, &report, now);
        attention.left(1);
        assert!(!attention.presence(true, now).known);
    }

    #[test]
    fn two_windows_read_as_one_person_and_a_renewal_is_no_news() {
        let mut attention = Attention::default();
        let now = Instant::now();
        let behind = proto::ReportPresence { in_front: false, last_input_ms: Some(5), lasts_ms: 0 };
        let front = proto::ReportPresence { in_front: true, last_input_ms: Some(3), lasts_ms: 0 };
        attention.report(1, &behind, now);
        attention.report(2, &front, now);
        let told = attention.presence_changed(true, now).expect("the first report is news");
        assert_eq!((told.in_front, told.last_input_ms), (true, Some(5)));
        attention.report(2, &front, now + Duration::from_secs(1));
        assert_eq!(attention.presence_changed(true, now + Duration::from_secs(1)), None);
        assert_eq!(attention.expire(now), Some(now + REPORT_LASTS));
    }

    #[test]
    fn a_report_cannot_claim_to_stand_for_ever() {
        let mut attention = Attention::default();
        let now = Instant::now();
        let forever = proto::ReportPresence { lasts_ms: u64::MAX, ..Default::default() };
        attention.report(1, &forever, now);
        assert_eq!(attention.expire(now), Some(now + LONGEST_REPORT));
    }
}
