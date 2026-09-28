//! What an agent says about its own state, which outranks the screen rules while it is fresh,
//! and whether the rules can still read the agent's screen at all.
//!
//! A harness that can say it is working, waiting on you or idle says so through a hook, and
//! that is worth more than any rule: a harness update that rewords its screen breaks the rules
//! and not the hook. The rules stay for harnesses with no hooks, and for panes where they are
//! not installed, and take over again whenever a report stops counting.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::detector::CarriedReport;
use crate::{Agent, State};

/// A working report stops counting after this long without a byte of output. A working agent
/// animates something - Claude Code a spinner and a timer - and one interrupted mid-turn, which
/// no hook reports, sits still at its prompt. This asks only that the screen move, not that any
/// rule match it, so it holds when the rules do not.
pub(crate) const QUIET: Duration = Duration::from_secs(10);

/// A blocked or idle report the rules have read the same way stops counting once they have read
/// something else for this long. No hook says a prompt went: Esc and a denial run none, and an
/// approved tool runs none until it ends. What the rules confirmed and then stopped seeing has
/// gone. The wait lets a prompt finish drawing.
///
/// A prompt on screen also sets a working report aside: one sub-agent can ask permission while
/// another's tool calls go on reporting working, and the prompt is still waiting on you. A
/// report that came after the prompt drew is set aside at once, and one that came before it
/// after this wait.
pub(crate) const DISAGREE: Duration = Duration::from_secs(2);

/// A blocked or idle report the rules have never read the same way stops counting once the pane
/// has produced output in this many seconds running since the report came. Such a report is all
/// there is to go on for a prompt the rules cannot read, and a prompt waiting on you sits still,
/// while an approved tool, or a background task the idle report knows nothing of, keeps the
/// screen moving; the rules read that screen instead.
pub(crate) const RESTLESS_SECONDS: usize = 3;

/// How long a report from an agent the pane is not yet known to run waits to be confirmed: a
/// hook can fire before detection's first probe of a new process.
pub(crate) const UNCONFIRMED: Duration = Duration::from_secs(2);

/// How long the rules must fail to read a screen that keeps changing before the pane says so.
pub(crate) const DRIFT: Duration = Duration::from_mins(1);

/// A change to the screen this soon after input was written is taken for its echo: someone
/// typing into an agent moves its screen, and that is neither the agent working nor its screen
/// going unread.
pub(crate) const ECHO: Duration = Duration::from_millis(500);

/// How many of those seconds the screen must have changed in: a screen that sits still is idle
/// by any reading, and never counts as unread.
const DRIFT_ACTIVE_SECONDS: usize = 30;

/// What an agent last said about its own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfReport {
    pub(crate) agent: Agent,
    pub(crate) state: State,
    pub(crate) at: Instant,
    /// Whether the rules have read the state reported since it came, or were reading it then.
    pub(crate) confirmed: bool,
}

/// How far along drift is, for another process to go on from: how long ago each of its spans
/// began, and the seconds of the last [`DRIFT`] in which the pane produced output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drift {
    pub rules_idle_ago: Option<Duration>,
    pub unmatched_ago: Option<Duration>,
    pub working_ago: Option<Duration>,
    pub active_ago: Vec<Duration>,
}

#[derive(Debug, Default)]
pub(crate) struct Reporting {
    report: Option<SelfReport>,
    last_content_seq: Option<u64>,
    /// When the content count was last taken.
    last_tick: Option<Instant>,
    last_output_at: Option<Instant>,
    /// The start of each second, of the last [`DRIFT`], in which the pane produced output.
    active_seconds: VecDeque<Instant>,
    /// What the rules read the last screen as, and since when every screen they read came to
    /// that.
    reading: Option<(State, Instant)>,
    /// Since when every screen the rules read came to a blocker they can see: a prompt, rather
    /// than a guess from the absence of anything else.
    blocker_since: Option<Instant>,
    /// Since when no rule matched any screen they read.
    unmatched_since: Option<Instant>,
    /// Since when a working report has counted without a break.
    working_since: Option<Instant>,
    unreadable: bool,
}

impl Reporting {
    pub(crate) fn report(&mut self, agent: Agent, state: State, now: Instant) {
        let confirmed = self.reading.is_some_and(|(read, _)| read == state);
        self.report = Some(SelfReport { agent, state, at: now, confirmed });
    }

    /// Takes the pane's content count, each tick while an agent is identified, and when input
    /// was last written to it. A change that input could have echoed since the last tick is not
    /// the agent's output.
    pub(crate) fn output(&mut self, content_seq: u64, input_at: Option<Instant>, now: Instant) {
        let since = self.last_tick.replace(now).unwrap_or(now);
        let echoed = input_at.is_some_and(|at| at + ECHO >= since);
        if !echoed && self.last_content_seq.is_some_and(|seen| seen != content_seq) {
            self.last_output_at = Some(now);
            let second = Duration::from_secs(1);
            if self.active_seconds.back().is_none_or(|&start| now.duration_since(start) >= second) {
                self.active_seconds.push_back(now);
            }
        }
        self.last_content_seq = Some(content_seq);
        while self.active_seconds.front().is_some_and(|&start| now.duration_since(start) > DRIFT) {
            self.active_seconds.pop_front();
        }
    }

    /// What the rules made of a screen they read: its state, whether any rule matched, and
    /// whether the rule that decided saw the state itself on screen.
    pub(crate) fn rules(&mut self, state: State, matched: bool, visible: bool, now: Instant) {
        if self.reading.is_none_or(|(read, _)| read != state) {
            self.reading = Some((state, now));
        }
        if state == State::Blocked && visible {
            self.blocker_since.get_or_insert(now);
        } else {
            self.blocker_since = None;
        }
        if let Some(report) = self.report.as_mut().filter(|report| report.state == state) {
            report.confirmed = true;
        }
        if matched {
            self.unmatched_since = None;
        } else {
            self.unmatched_since.get_or_insert(now);
        }
    }

    /// The pane's agent changed: what was learned of the last one's screen is no guide.
    pub(crate) fn agent_changed(&mut self) {
        self.reading = None;
        self.blocker_since = None;
        self.unmatched_since = None;
        self.working_since = None;
        self.active_seconds.clear();
        self.unreadable = false;
    }

    /// The state the agent reported, while that still counts for `agent`, the agent the pane
    /// runs. A report stops counting when a newer one comes, when the pane's agent is not the
    /// one that reported (after [`UNCONFIRMED`] for one not yet identified), when the agent's
    /// process has exited, for working after [`QUIET`] without output, and for blocked or idle
    /// after [`DISAGREE`] of the rules reading otherwise once they have read it the same way, or
    /// else after [`RESTLESS_SECONDS`] of output running. A working report is set aside, and
    /// counts again after, while the rules have read a prompt on screen for [`DISAGREE`], or
    /// from the moment it comes if the prompt was on screen already.
    pub(crate) fn in_force(
        &mut self,
        agent: Option<&Agent>,
        exited: bool,
        now: Instant,
    ) -> Option<State> {
        let report = self.report.as_ref()?;
        let confirmed = agent == Some(&report.agent);
        let quiet_since = self.last_output_at.map_or(report.at, |at| at.max(report.at));
        let stale = if report.state == State::Working {
            now.duration_since(quiet_since) >= QUIET
        } else if report.confirmed {
            self.reading.is_some_and(|(read, since)| {
                read != report.state && now.duration_since(since) >= DISAGREE
            })
        } else {
            self.restless_since(report.at, now)
        };
        let unconfirmed = !confirmed && now.duration_since(report.at) >= UNCONFIRMED;
        if exited || stale || unconfirmed {
            self.report = None;
            return None;
        }
        let prompted = self
            .blocker_since
            .is_some_and(|since| since <= report.at || now.duration_since(since) >= DISAGREE);
        if report.state == State::Working && prompted {
            return None;
        }
        confirmed.then_some(report.state)
    }

    /// Since when every screen the rules read came to idle.
    fn rules_idle_since(&self) -> Option<Instant> {
        self.reading.filter(|&(read, _)| read == State::Idle).map(|(_, since)| since)
    }

    /// Whether the pane has produced output in each of the last [`RESTLESS_SECONDS`] seconds,
    /// all of them after `since`.
    fn restless_since(&self, since: Instant, now: Instant) -> bool {
        // A second is recorded at the first tick with output a second or more after the last,
        // so output that never stops records them 1.0 to 1.3 s apart at the 300 ms tick. Wider
        // than that, some second in between had none. This takes ticks to come under 500 ms
        // apart. Each is scheduled from when the last ran, so on a machine loaded enough to run
        // them 200 ms late a run can break under output that never stopped, and a report the
        // rules never read holds longer: the error is on the side of the report.
        let gap = Duration::from_millis(1500);
        let mut next = now;
        let mut running = 0;
        for &start in self.active_seconds.iter().rev() {
            if start < since || next.duration_since(start) > gap {
                break;
            }
            running += 1;
            next = start;
        }
        running >= RESTLESS_SECONDS
    }

    /// Whether the rules have stopped reading `agent`'s screen, given the state its own report
    /// holds it at. Either a working report has counted for [`DRIFT`] while the rules read every
    /// screen as idle, or with no report no rule has matched for that long; and either way the
    /// screen changed in most of those seconds, as a working agent's does.
    pub(crate) fn unreadable(
        &mut self,
        agent: Option<&Agent>,
        reported: Option<State>,
        now: Instant,
    ) -> bool {
        if agent.is_none() {
            self.agent_changed();
            return false;
        }
        if reported == Some(State::Working) {
            self.working_since.get_or_insert(now);
        } else {
            self.working_since = None;
        }
        let held = |since: Option<Instant>| since.is_some_and(|at| now.duration_since(at) >= DRIFT);
        let moving = self.active_seconds.len() >= DRIFT_ACTIVE_SECONDS;
        let contradicted = held(self.working_since) && held(self.rules_idle_since());
        let unmatched = reported.is_none() && held(self.unmatched_since);
        self.unreadable = moving && (contradicted || unmatched);
        self.unreadable
    }

    /// What another process needs to go on: the report, and how long ago the pane last produced
    /// output. What the rules read is learned again, from the screens the new process sees; that
    /// they had read the report's state is carried, since a report they confirmed lets go on a
    /// still screen where one they never read would not.
    pub(crate) fn carried(&self, now: Instant) -> (Option<CarriedReport>, Option<Duration>) {
        let report = self.report.as_ref().map(|report| CarriedReport {
            agent: report.agent.clone(),
            state: report.state,
            ago: now.duration_since(report.at),
            confirmed: report.confirmed,
        });
        (report, self.last_output_at.map(|at| now.duration_since(at)))
    }

    /// Where drift stands, so the process a pane is handed to neither clears `unreadable` while
    /// it learns the screen again nor says it a second time.
    pub(crate) fn drift(&self, now: Instant) -> Drift {
        let ago = |at: Instant| now.saturating_duration_since(at);
        Drift {
            rules_idle_ago: self.rules_idle_since().map(ago),
            unmatched_ago: self.unmatched_since.map(ago),
            working_ago: self.working_since.map(ago),
            active_ago: self.active_seconds.iter().copied().map(ago).collect(),
        }
    }

    /// How long a prompt the rules can see has been on screen, which a working report is set
    /// aside for.
    pub(crate) fn blocker_ago(&self, now: Instant) -> Option<Duration> {
        self.blocker_since.map(|since| now.saturating_duration_since(since))
    }

    pub(crate) fn resumed(
        report: Option<CarriedReport>,
        output_ago: Option<Duration>,
        drift: Drift,
        blocker_ago: Option<Duration>,
        now: Instant,
    ) -> Reporting {
        let at = |ago: Duration| now.checked_sub(ago);
        let report = report.and_then(|report| {
            Some(SelfReport {
                agent: report.agent,
                state: report.state,
                at: at(report.ago)?,
                confirmed: report.confirmed,
            })
        });
        Reporting {
            report,
            last_output_at: output_ago.and_then(at),
            reading: drift.rules_idle_ago.and_then(at).map(|since| (State::Idle, since)),
            unmatched_since: drift.unmatched_ago.and_then(at),
            working_since: drift.working_ago.and_then(at),
            active_seconds: drift.active_ago.into_iter().filter_map(at).collect(),
            blocker_since: blocker_ago.and_then(at),
            ..Reporting::default()
        }
    }
}
