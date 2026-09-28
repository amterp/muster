//! Agent detection in each pane (MIP-3 section 8): muster-detect, driven by the pane's reader.
//!
//! The reader owns its pane's [`Detection`] and ticks it on the poll's timeout, so detection
//! needs no thread of its own. A tick reads the pane's terminal under the pane's lock a piece at
//! a time and probes processes with no lock held. What it publishes goes to the publisher, like
//! every other report, so the reader never waits on the session.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_detect::{
    Agent, Carried, CarriedReport, Detector, Drift, Manifests, Pane, Progress, Publication, State,
    System,
};

use crate::pane::PaneIo;

/// The manifests every pane is detected by: built in, sent by the app, and a person's overrides.
#[derive(Debug)]
pub(crate) struct Detecting {
    /// None until the first load lands.
    manifests: Mutex<Option<Arc<Manifests>>>,
    /// `~/.muster/agent-detection/`, read again whenever the manifests are.
    overrides: Option<PathBuf>,
}

impl Detecting {
    /// Starts loading the manifests on a thread of their own. Compiling every manifest's
    /// patterns takes tens of milliseconds, which a daemon's first answer must not wait for;
    /// until they land, panes simply are not detected yet.
    pub(crate) fn start(overrides: Option<PathBuf>) -> Arc<Detecting> {
        let detecting = Arc::new(Detecting { manifests: Mutex::new(None), overrides });
        let loading = Arc::clone(&detecting);
        let started =
            std::thread::Builder::new().name("detect load".to_string()).spawn(move || {
                let loaded = load(&[], loading.overrides.as_deref());
                // An app's manifests that arrived first include these, and win.
                poison::lock(&loading.manifests, "daemon.detect.manifests")
                    .get_or_insert_with(|| Arc::new(loaded));
            });
        if let Err(error) = started {
            log::error(
                "daemon.detect.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "no pane's agent is detected until the app sends its manifests",
                    "check" => "whether the daemon is out of threads",
                },
            );
        }
        detecting
    }

    pub(crate) fn manifests(&self) -> Option<Arc<Manifests>> {
        poison::lock(&self.manifests, "daemon.detect.manifests").clone()
    }

    /// Compiles the manifests again, with the app's. Tens of milliseconds, and it reads the
    /// override directory, which can be on a mount that hangs: never done under the session
    /// lock.
    pub(crate) fn load(&self, app: &[(String, String)]) -> Manifests {
        load(app, self.overrides.as_deref())
    }

    /// Puts manifests from [`Detecting::load`] in use, and returns the agents whose manifest
    /// changed: only their panes need detecting afresh.
    pub(crate) fn adopt(&self, loaded: Manifests) -> Vec<Agent> {
        let mut manifests = poison::lock(&self.manifests, "daemon.detect.manifests");
        // Before the first load, nothing has been detected by anything.
        let changed = manifests.as_ref().map(|current| loaded.changed_since(current));
        if changed.as_ref().is_none_or(|changed| !changed.is_empty()) {
            *manifests = Some(Arc::new(loaded));
        }
        changed.unwrap_or_default()
    }
}

fn load(app: &[(String, String)], overrides: Option<&std::path::Path>) -> Manifests {
    let started = Instant::now();
    let (manifests, warnings) = Manifests::load(app, overrides);
    for warning in &warnings {
        // A warning's text already says what it costs and what to do.
        log::warn("daemon.detect.manifest_ignored", fields! { "warning" => warning });
    }
    log::info(
        "daemon.detect.loaded",
        fields! {
            "from_app" => app.len(),
            "ignored" => warnings.len(),
            "ms" => started.elapsed().as_millis(),
        },
    );
    manifests
}

/// One pane's detection, owned by its reader.
#[derive(Debug)]
pub(crate) struct Detection {
    detector: Detector,
    /// OSC 9 progress, which manifests match as the program wrote it: the terminal hands it on
    /// parsed, so the reader scans the raw output for it too.
    progress: Progress,
    due: Instant,
}

impl Detection {
    /// Detection for a pane whose shell is `shell`, when this daemon started one.
    pub(crate) fn new(shell: Option<i32>, now: Instant) -> Detection {
        let shell = shell.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
        Detection {
            detector: Detector::new(shell, now),
            progress: Progress::default(),
            due: now + Detector::FIRST_TICK,
        }
    }

    /// Where this pane's detection stands, for the daemon it is handed to.
    pub(crate) fn carried(&self, now: Instant) -> proto::handoff::Detection {
        let carried = self.detector.carried(now);
        let millis = |at: Duration| u32::try_from(at.as_millis()).unwrap_or(u32::MAX);
        let (emitted_agent, emitted_state) =
            carried.emitted.as_ref().map_or((None, proto::AgentState::Unknown), recorded);
        let (concluded_agent, concluded_state) =
            carried.concluded.as_ref().map_or((None, proto::AgentState::Unknown), recorded);
        let report = carried.report.as_ref();
        proto::handoff::Detection {
            agent: carried.agent.as_ref().map(|agent| agent.id().to_string()),
            misses: carried.misses.into(),
            state: agent_state(carried.state).into(),
            visible: carried.visible,
            emitted: carried.emitted.is_some(),
            emitted_agent,
            emitted_state: emitted_state.into(),
            grace_left_ms: carried.grace_left.map(millis),
            idle_seen_ms_ago: carried.idle_seen_ago.map(millis),
            idle_confirmations: carried.idle_confirmations.into(),
            foreground_group: carried.foreground_group,
            probed: carried.probed,
            shell_clear_pending: carried.shell_clear_pending,
            shell_exit_reported: carried.shell_exit_reported,
            title_pending: carried.title_pending,
            progress: self.progress.get().to_string(),
            concluded: carried.concluded.is_some(),
            concluded_agent,
            concluded_state: concluded_state.into(),
            report_agent: report.map(|report| report.agent.id().to_string()),
            report_state: report
                .map_or(proto::AgentState::Unknown, |report| agent_state(report.state))
                .into(),
            report_ms_ago: report.map_or(0, |report| millis(report.ago)),
            report_confirmed: report.is_some_and(|report| report.confirmed),
            output_ms_ago: carried.output_ago.map(millis),
            emitted_reported: carried.emitted.as_ref().is_some_and(|emitted| emitted.reported),
            emitted_unreadable: carried.emitted.as_ref().is_some_and(|emitted| emitted.unreadable),
            rules_idle_ms_ago: carried.drift.rules_idle_ago.map(millis),
            unmatched_ms_ago: carried.drift.unmatched_ago.map(millis),
            working_ms_ago: carried.drift.working_ago.map(millis),
            active_ms_ago: carried.drift.active_ago.iter().copied().map(millis).collect(),
            blocker_ms_ago: carried.blocker_ago.map(millis),
        }
    }

    /// Detection going on from where the daemon that handed the pane over left it.
    /// `title_writes` is the pane's count in this daemon.
    pub(crate) fn resumed(
        shell: Option<i32>,
        carried: &proto::handoff::Detection,
        now: Instant,
        title_writes: u64,
    ) -> Detection {
        let shell = shell.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
        let millis = |ms: u32| Duration::from_millis(ms.into());
        let emitted = carried.emitted.then(|| Publication {
            agent: carried.emitted_agent.as_deref().map(Agent::new),
            state: state_of(carried.emitted_state()),
            reported: carried.emitted_reported,
            unreadable: carried.emitted_unreadable,
        });
        let concluded = carried.concluded.then(|| Publication {
            agent: carried.concluded_agent.as_deref().map(Agent::new),
            state: state_of(carried.concluded_state()),
            reported: false,
            unreadable: false,
        });
        let report = carried.report_agent.as_deref().map(|agent| CarriedReport {
            agent: Agent::new(agent),
            state: state_of(carried.report_state()),
            ago: millis(carried.report_ms_ago),
            confirmed: carried.report_confirmed,
        });
        let carried_here = Carried {
            agent: carried.agent.as_deref().map(Agent::new),
            misses: u8::try_from(carried.misses).unwrap_or(u8::MAX),
            state: state_of(carried.state()),
            visible: carried.visible,
            emitted,
            grace_left: carried.grace_left_ms.map(millis),
            idle_seen_ago: carried.idle_seen_ms_ago.map(millis),
            idle_confirmations: u8::try_from(carried.idle_confirmations).unwrap_or(u8::MAX),
            foreground_group: carried.foreground_group,
            probed: carried.probed,
            shell_clear_pending: carried.shell_clear_pending,
            shell_exit_reported: carried.shell_exit_reported,
            title_pending: carried.title_pending,
            concluded,
            report,
            output_ago: carried.output_ms_ago.map(millis),
            drift: Drift {
                rules_idle_ago: carried.rules_idle_ms_ago.map(millis),
                unmatched_ago: carried.unmatched_ms_ago.map(millis),
                working_ago: carried.working_ms_ago.map(millis),
                active_ago: carried.active_ms_ago.iter().copied().map(millis).collect(),
            },
            blocker_ago: carried.blocker_ms_ago.map(millis),
        };
        let mut progress = Progress::default();
        if !carried.progress.is_empty() {
            progress.observe(format!("\x1b]9;{}\x07", carried.progress).as_bytes());
        }
        Detection {
            detector: Detector::resumed(shell, carried_here, now, title_writes),
            progress,
            due: now + Detector::FIRST_TICK,
        }
    }

    /// What the pane's agent says about its own state, ticked on at once.
    pub(crate) fn report(&mut self, agent: &str, state: State, now: Instant) {
        self.detector.report(Agent::new(agent), state, now);
        self.due = now;
    }

    /// Every chunk of the pane's output, in order.
    pub(crate) fn observe(&mut self, bytes: &[u8]) {
        self.progress.observe(bytes);
    }

    /// When the next tick is due.
    pub(crate) fn due(&self) -> Instant {
        self.due
    }

    /// Detects, and says what to publish if anything changed.
    pub(crate) fn tick(
        &mut self,
        io: &PaneIo,
        detecting: &Detecting,
        now: Instant,
    ) -> Option<Publication> {
        // Taken before the manifests are read, so a reset asked for by a reload runs against
        // the manifests that reload put in use, never the ones it replaced.
        let reset = io.take_detection_reset();
        let Some(manifests) = detecting.manifests() else {
            // Nothing has been detected yet, so a reset has nothing to undo.
            self.due = now + Detector::FIRST_TICK;
            return None;
        };
        if reset {
            self.detector.reset();
        }
        let mut observed = Observed { io, progress: &mut self.progress };
        let tick = self.detector.tick(now, &mut observed, &System, &manifests);
        self.due = now + tick.next;
        tick.publication
    }
}

/// A publication as the pane's record holds it.
pub(crate) fn recorded(publication: &Publication) -> (Option<String>, proto::AgentState) {
    let state = agent_state(publication.state);
    (publication.agent.as_ref().map(|agent| agent.id().to_string()), state)
}

fn agent_state(state: State) -> proto::AgentState {
    match state {
        State::Working => proto::AgentState::Working,
        State::Blocked => proto::AgentState::Blocked,
        State::Idle => proto::AgentState::Idle,
        State::Unknown => proto::AgentState::Unknown,
    }
}

pub(crate) fn state_of(state: proto::AgentState) -> State {
    match state {
        proto::AgentState::Working => State::Working,
        proto::AgentState::Blocked => State::Blocked,
        proto::AgentState::Idle => State::Idle,
        proto::AgentState::Unknown => State::Unknown,
    }
}

/// A pane as detection reads it.
struct Observed<'a> {
    io: &'a PaneIo,
    progress: &'a mut Progress,
}

impl Pane for Observed<'_> {
    fn foreground_group(&self) -> Option<u32> {
        self.io.foreground_group().and_then(|group| u32::try_from(group).ok())
    }

    fn content_seq(&self) -> u64 {
        self.io.screen().content_seq()
    }

    fn input_at(&self) -> Option<Instant> {
        self.io.input_at()
    }

    fn screen_text(&mut self) -> String {
        let screen = self.io.screen();
        let terminal = screen.terminal();
        terminal.text(0, terminal.rows().saturating_sub(1))
    }

    fn title(&self) -> String {
        self.io.screen().terminal().title()
    }

    fn title_writes(&self) -> u64 {
        self.io.screen().title_writes()
    }

    fn progress(&self) -> String {
        self.progress.get().to_string()
    }

    fn clear_progress(&mut self) {
        self.progress.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of a pane's detection, none at its default, so a field this daemon drops or
    /// mixes up on its way through reads differently coming back.
    fn handed_over() -> proto::handoff::Detection {
        proto::handoff::Detection {
            agent: Some("claude".to_string()),
            misses: 1,
            state: proto::AgentState::Blocked.into(),
            visible: true,
            emitted: true,
            emitted_agent: Some("claude".to_string()),
            emitted_state: proto::AgentState::Blocked.into(),
            grace_left_ms: Some(1200),
            idle_seen_ms_ago: Some(300),
            idle_confirmations: 2,
            foreground_group: Some(4242),
            probed: true,
            shell_clear_pending: true,
            shell_exit_reported: true,
            title_pending: true,
            progress: "4;1;40".to_string(),
            concluded: true,
            concluded_agent: Some("claude".to_string()),
            concluded_state: proto::AgentState::Idle.into(),
            report_agent: Some("claude".to_string()),
            report_state: proto::AgentState::Working.into(),
            report_ms_ago: 500,
            output_ms_ago: Some(50),
            emitted_reported: true,
            emitted_unreadable: true,
            rules_idle_ms_ago: Some(40_000),
            unmatched_ms_ago: Some(20_000),
            working_ms_ago: Some(30_000),
            active_ms_ago: vec![2000, 1000],
            report_confirmed: true,
            blocker_ms_ago: Some(3000),
        }
    }

    #[test]
    fn a_pane_handed_over_is_handed_on_as_it_came() {
        let now = Instant::now();
        let carried = handed_over();
        let resumed = Detection::resumed(Some(100), &carried, now, 7);
        assert_eq!(resumed.carried(now), carried);
    }
}
