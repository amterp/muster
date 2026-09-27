//! When a pane's agent and state are published: the per-pane detection loop.
//!
//! Ported from herdr v0.8.0 `src/pane.rs` (the detection task and the process-probe
//! scheduling around it) and `src/pane/agent_detection.rs` (Apache-2.0), and changed: it is a
//! synchronous state machine the caller ticks, with the time passed in, rather than a task
//! that sleeps; hook authority, graceful release, the handoff-restored agent and the host
//! theme are gone; a publication is only an agent and a state, emitted when either changes;
//! and the title's reset on an agent change is kept here, counted in the terminal's title writes,
//! since the title lives in the caller's terminal.
//!
//! What the loop is for is flicker. A screen is read a few times a second, and an agent's
//! chrome is not drawn atomically, so a verdict taken at face value would bounce - working,
//! idle for a frame, working again. The rules below are herdr's, each earned by an agent
//! that did that: a new agent gets three seconds before its screen counts, working only
//! becomes idle once the idle has held (unless the agent drew its idle chrome), and an agent
//! is only forgotten after six probes in a row fail to find it.

use std::time::{Duration, Instant};

use crate::identify::probe;
use crate::manifest::Detection;
use crate::process::Processes;
use crate::{Agent, Input, Manifests, State, screen_text, title};

pub(crate) mod reporting;

use reporting::Reporting;

const TICK_UNIDENTIFIED: Duration = Duration::from_millis(500);
const TICK_IDENTIFIED: Duration = Duration::from_millis(300);
const PENDING_IDLE_RECHECK: Duration = Duration::from_millis(100);
const PENDING_IDLE_CONFIRMATIONS: u8 = 3;
const PENDING_IDLE_CAP: Duration = Duration::from_millis(700);
const STARTUP_GRACE: Duration = Duration::from_secs(3);

const AGENT_MISS_CONFIRMATION_ATTEMPTS: u8 = 6;
const PROCESS_RECHECK_IDENTIFIED: Duration = Duration::from_secs(5);
const PROCESS_RECHECK_MISSING_FOREGROUND_GROUP: Duration = Duration::from_secs(30);
const PROCESS_ACQUISITION_WINDOW: Duration = Duration::from_secs(8);
const PROCESS_ACQUISITION_FAST_WINDOW: Duration = Duration::from_millis(1500);
const PROCESS_ACQUISITION_FAST_RECHECK: Duration = Duration::from_millis(500);
const PROCESS_ACQUISITION_SLOW_RECHECK: Duration = Duration::from_secs(2);
const PROCESS_ACQUISITION_IDLE_RESET: Duration = Duration::from_secs(2);

/// What the detector reads from a pane. The daemon implements it over its terminal and PTY;
/// a test implements it over whatever it likes.
pub trait Pane {
    /// The terminal's foreground process group, or none if the terminal will not say.
    fn foreground_group(&self) -> Option<u32>;

    /// A count that moves whenever the screen may have: for every non-empty read from the PTY,
    /// and for every resize, which rewraps the screen without a byte of output from an agent
    /// that does not redraw on SIGWINCH. An unchanged count lets an idle pane skip its screen
    /// read.
    fn content_seq(&self) -> u64;

    /// The active screen's rows, as `muster_vt::Terminal::text(0, rows - 1)` reads them.
    fn screen_text(&mut self) -> String;

    /// The terminal's title, as the program in the pane last set it.
    fn title(&self) -> String;

    /// How many times a program has set the title (OSC 0 and 2), counting a write that repeats
    /// the title it already had.
    fn title_writes(&self) -> u64;

    /// The last OSC 9 payload (`crate::Progress::get`).
    fn progress(&self) -> String;

    /// Forgets the OSC 9 payload (`crate::Progress::clear`), so a new agent starts without
    /// the last one's.
    fn clear_progress(&mut self);
}

/// The outcome of a tick: something to publish, perhaps, and when to tick next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub publication: Option<Publication>,
    pub next: Duration,
}

/// A pane's agent and its state, published because one of them changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub agent: Option<Agent>,
    pub state: State,
    /// The state is the agent's own report rather than the screen rules' reading.
    pub reported: bool,
    /// The screen rules have stopped reading this agent's screen ([`reporting`]).
    pub unreadable: bool,
}

impl Publication {
    /// What the screen rules concluded, before any report of the agent's own.
    fn concluded(agent: Option<Agent>, state: State) -> Publication {
        Publication { agent, state, reported: false, unreadable: false }
    }
}

/// One pane's detection. Tick it `FIRST_TICK` after the pane starts, and then after each
/// tick's `next`.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "herdr's flags, kept as herdr named them so its loop ports as it was"
)]
pub struct Detector {
    shell: u32,
    presence: Presence,
    published: PublishState,
    last_process_check: Instant,
    last_foreground_group: Option<u32>,
    has_process_probe: bool,
    acquisition_started_at: Option<Instant>,
    last_content_change_at: Option<Instant>,
    pending_foreground_shell_clear: bool,
    foreground_shell_exit_reported: bool,
    last_detection_text: String,
    last_screen_scan_content_seq: Option<u64>,
    startup_grace_until: Option<Instant>,
    pending_idle: PendingIdle,
    /// How many title writes the pane had seen when the agent last changed. The title reads as
    /// empty until the next write, even one that repeats it: herdr dropped its copy of the title
    /// here and took the next OSC 0 or 2 as it came, and this title is the caller's.
    title_writes_at_change: Option<u64>,
    last_emitted: Option<Publication>,
    /// What the screen rules last concluded, which [`Detector::tick`] publishes unless the
    /// agent's own report outranks it.
    last_concluded: Option<Publication>,
    reporting: Reporting,
    /// A reset forgot what was known, so the next tick publishes what it finds, even when it
    /// finds nothing that would count as a change. Otherwise a pane whose agent went with its
    /// manifest would go on showing the old agent's last state.
    owed: bool,
}

impl Detector {
    /// herdr's first tick came 50 ms after the pane started, giving the shell a moment to
    /// become the foreground.
    pub const FIRST_TICK: Duration = Duration::from_millis(50);

    /// A detector for the pane whose process is `shell`, created at `now`.
    pub fn new(shell: u32, now: Instant) -> Detector {
        Detector {
            shell,
            presence: Presence::default(),
            published: PublishState { state: State::Idle, visible: false },
            last_process_check: now,
            last_foreground_group: None,
            has_process_probe: false,
            acquisition_started_at: None,
            last_content_change_at: None,
            pending_foreground_shell_clear: false,
            foreground_shell_exit_reported: false,
            last_detection_text: String::new(),
            last_screen_scan_content_seq: None,
            startup_grace_until: None,
            pending_idle: PendingIdle::default(),
            title_writes_at_change: None,
            last_emitted: None,
            last_concluded: None,
            reporting: Reporting::default(),
            owed: false,
        }
    }

    /// Starts over, as if the pane had just opened - for when the manifests change and what
    /// was known was known under the old ones. Tick straight after: that tick publishes the
    /// pane's agent and state as they now are, unless they are what was last published.
    pub fn reset(&mut self) {
        self.presence = Presence::default();
        self.published = PublishState { state: State::Unknown, visible: false };
        self.last_foreground_group = None;
        self.has_process_probe = false;
        self.acquisition_started_at = None;
        self.last_content_change_at = None;
        self.pending_foreground_shell_clear = false;
        self.foreground_shell_exit_reported = false;
        self.last_detection_text.clear();
        self.last_screen_scan_content_seq = None;
        self.startup_grace_until = None;
        self.pending_idle.clear();
        self.owed = true;
    }

    /// Where this detector stands, for another process to go on from with [`Detector::resumed`]:
    /// a daemon handing its panes to another.
    pub fn carried(&self, now: Instant) -> Carried {
        let until = |at: Instant| at.saturating_duration_since(now);
        let (report, output_ago) = self.reporting.carried(now);
        Carried {
            agent: self.presence.current.clone(),
            misses: self.presence.consecutive_misses,
            state: self.published.state,
            visible: self.published.visible,
            emitted: self.last_emitted.clone(),
            grace_left: self.startup_grace_until.map(until),
            idle_seen_ago: self.pending_idle.started_at.map(|at| now.saturating_duration_since(at)),
            idle_confirmations: self.pending_idle.confirmations,
            foreground_group: self.last_foreground_group,
            probed: self.has_process_probe,
            shell_clear_pending: self.pending_foreground_shell_clear,
            shell_exit_reported: self.foreground_shell_exit_reported,
            title_pending: self.title_writes_at_change.is_some(),
            concluded: self.last_concluded.clone(),
            report,
            output_ago,
        }
    }

    /// A detector going on from `carried`, for the pane whose process is `shell`, at `now`.
    /// `title_writes` is the pane's count here, which a title still pending is counted from.
    /// The screen is read afresh at the next tick, which finds the same agent and so publishes
    /// nothing unless its state has moved.
    pub fn resumed(shell: u32, carried: Carried, now: Instant, title_writes: u64) -> Detector {
        let mut detector = Detector::new(shell, now);
        detector.presence = Presence { current: carried.agent, consecutive_misses: carried.misses };
        detector.published = PublishState { state: carried.state, visible: carried.visible };
        detector.last_emitted = carried.emitted;
        detector.startup_grace_until = carried.grace_left.map(|left| now + left);
        detector.pending_idle = PendingIdle {
            started_at: carried.idle_seen_ago.and_then(|ago| now.checked_sub(ago)),
            confirmations: carried.idle_confirmations,
        };
        detector.last_foreground_group = carried.foreground_group;
        detector.has_process_probe = carried.probed;
        detector.pending_foreground_shell_clear = carried.shell_clear_pending;
        detector.foreground_shell_exit_reported = carried.shell_exit_reported;
        detector.title_writes_at_change = carried.title_pending.then_some(title_writes);
        detector.last_concluded = carried.concluded;
        detector.reporting = Reporting::resumed(carried.report, carried.output_ago, now);
        detector
    }

    pub fn agent(&self) -> Option<&Agent> {
        self.presence.current.as_ref()
    }

    /// What the agent says about its own state, which outranks the screen rules while it counts
    /// ([`reporting`]). Published at the next tick.
    pub fn report(&mut self, agent: Agent, state: State, now: Instant) {
        self.reporting.report(agent, state, now);
    }

    /// What the rules last concluded, with the agent's own report laid over it while that counts.
    fn effective(&mut self, now: Instant) -> Option<Publication> {
        let concluded = self.last_concluded.clone()?;
        let agent = self.presence.current.as_ref();
        let exited = self.pending_foreground_shell_clear;
        let reported = self.reporting.in_force(agent, exited, now);
        let unreadable = self.reporting.unreadable(agent, reported, now);
        Some(Publication {
            state: reported.unwrap_or(concluded.state),
            reported: reported.is_some(),
            unreadable,
            ..concluded
        })
    }

    pub fn tick(
        &mut self,
        now: Instant,
        pane: &mut impl Pane,
        processes: &impl Processes,
        manifests: &Manifests,
    ) -> Tick {
        let concluded = self.step(now, pane, processes, manifests);
        if std::mem::take(&mut self.owed) && concluded.is_none() {
            self.conclude(self.presence.current.clone(), self.published.state);
        }
        if self.presence.current.is_some() {
            self.reporting.output(pane.content_seq(), now);
        }
        let publication = self.effective(now).and_then(|effective| self.emit(effective));
        let next = if self.pending_idle.active() {
            PENDING_IDLE_RECHECK
        } else if self.presence.current.is_none() {
            TICK_UNIDENTIFIED
        } else {
            TICK_IDENTIFIED
        };
        Tick { publication, next }
    }

    fn step(
        &mut self,
        now: Instant,
        pane: &mut impl Pane,
        processes: &impl Processes,
        manifests: &Manifests,
    ) -> Option<Publication> {
        let foreground = self.check_foreground(now, pane, processes, manifests);
        let mut emitted = foreground.emitted;
        let agent_changed = foreground.agent_changed;
        let agent = self.presence.current.clone();

        let process_exited = self.pending_foreground_shell_clear
            && agent.is_some()
            && !self.foreground_shell_exit_reported;

        if let Some(until) = self.startup_grace_until {
            if process_exited {
                self.startup_grace_until = None;
                self.last_screen_scan_content_seq = None;
                self.pending_idle.clear();
            } else {
                // The tick the grace runs out on is skipped too, as it was in herdr.
                if now >= until {
                    self.startup_grace_until = None;
                }
                self.pending_idle.clear();
                return emitted;
            }
        }

        let content_seq = agent.is_some().then(|| pane.content_seq());
        if should_skip_screen_read(ScreenReadInput {
            state: self.published.state,
            identified: agent.is_some(),
            pending_idle_active: self.pending_idle.active(),
            agent_changed,
            process_exited,
            content_seq,
            last_screen_scan_content_seq: self.last_screen_scan_content_seq,
        }) {
            return emitted;
        }

        let content = screen_text(&pane.screen_text());
        self.last_screen_scan_content_seq = content_seq;
        let content_changed = content != self.last_detection_text;
        self.last_detection_text.clone_from(&content);
        // herdr asked this of the screen alone, before reading the title and progress.
        let screen_only = Input { screen: &content, ..Input::default() };
        if manifests.detect(agent.as_ref(), screen_only).skip_state_update {
            self.pending_idle.clear();
            return emitted;
        }
        sync_content_change_acquisition(
            self.presence.current.is_some(),
            foreground.group_changed,
            content_changed,
            now,
            &mut self.acquisition_started_at,
            &mut self.last_content_change_at,
        );

        let detection = if process_exited {
            Detection { state: State::Idle, visible: true, skip_state_update: false, rule: None }
        } else {
            let title = self.current_title(pane);
            let progress = pane.progress();
            let detection = manifests.detect(
                agent.as_ref(),
                Input { screen: &content, title: &title, progress: &progress },
            );
            if detection.skip_state_update {
                self.pending_idle.clear();
                return emitted;
            }
            detection
        };

        if !process_exited {
            // Idle by the fallback is a reading, not a miss, for a manifest that has no idle rule
            // of its own: that fallback is how it reads idle.
            let by_fallback = detection.state == State::Idle
                && !agent.as_ref().is_some_and(|agent| manifests.has_rule_for(agent, State::Idle));
            self.reporting.rules(detection.state, detection.rule.is_some() || by_fallback, now);
        }
        let next = PublishState { state: detection.state, visible: detection.visible };
        if decide_transition(
            self.published,
            next,
            agent_changed,
            process_exited,
            now,
            &mut self.pending_idle,
        ) {
            self.published = next;
            if process_exited {
                self.foreground_shell_exit_reported = true;
            }
            emitted = self.conclude(agent, next.state).or(emitted);
        }
        emitted
    }

    /// Probes the foreground when something suggests it changed, and acts on what it finds.
    fn check_foreground(
        &mut self,
        now: Instant,
        pane: &mut impl Pane,
        processes: &impl Processes,
        manifests: &Manifests,
    ) -> Foreground {
        let foreground_group = pane.foreground_group();
        let group_changed = foreground_group_changed(foreground_group, self.last_foreground_group);
        let mut foreground = Foreground { group_changed, agent_changed: false, emitted: None };
        if !should_probe_foreground_job(ProbeInput {
            identified: self.presence.current.is_some(),
            foreground_group,
            last_foreground_group: self.last_foreground_group,
            has_process_probe: self.has_process_probe,
            acquisition_age: self
                .acquisition_started_at
                .map(|started| now.saturating_duration_since(started)),
            pending_foreground_shell_clear: self.pending_foreground_shell_clear,
            elapsed_since_process_check: now.saturating_duration_since(self.last_process_check),
        }) {
            return foreground;
        }

        self.last_process_check = now;
        let had_process_probe = self.has_process_probe;
        self.has_process_probe = true;
        let found = probe(self.shell, foreground_group, processes, manifests);
        let previous = self.presence.current.clone();
        let action = foreground_shell_agent_action(
            previous.as_ref(),
            found.agent.as_ref(),
            found.shell_in_foreground,
            self.foreground_shell_exit_reported,
        );
        let changed = self.apply(action, previous.as_ref(), found.agent.clone());
        self.last_foreground_group =
            process_group_for_change_tracking(foreground_group, found.group);
        if found.agent.is_some() {
            self.acquisition_started_at = None;
            self.last_content_change_at = None;
        } else if self.presence.current.is_none() && had_process_probe && group_changed {
            self.acquisition_started_at = Some(now);
        }
        if !changed {
            return foreground;
        }
        foreground.agent_changed = true;
        let agent = self.presence.current.clone();
        if agent != previous || action == ForegroundShellAgentAction::ReportReplacementProcess {
            self.reporting.agent_changed();
            self.pending_idle.clear();
            self.last_screen_scan_content_seq = None;
            // A new agent must not inherit the last one's title or progress.
            pane.clear_progress();
            self.title_writes_at_change = Some(pane.title_writes());
            if agent.is_some() {
                self.startup_grace_until = Some(now + STARTUP_GRACE);
                self.published = PublishState { state: State::Idle, visible: true };
                foreground.emitted = self.conclude(agent, State::Idle);
            } else {
                self.startup_grace_until = None;
            }
        }
        foreground
    }

    fn apply(
        &mut self,
        action: ForegroundShellAgentAction,
        previous: Option<&Agent>,
        found: Option<Agent>,
    ) -> bool {
        match action {
            ForegroundShellAgentAction::ReportReplacementProcess => {
                self.pending_foreground_shell_clear = false;
                self.foreground_shell_exit_reported = false;
                self.presence.observe(previous.cloned());
                true
            }
            ForegroundShellAgentAction::ReportProcessExit => {
                self.pending_foreground_shell_clear = true;
                false
            }
            ForegroundShellAgentAction::ClearAgent => {
                self.pending_foreground_shell_clear = false;
                self.foreground_shell_exit_reported = false;
                self.presence.clear()
            }
            ForegroundShellAgentAction::ObserveProbe => {
                self.pending_foreground_shell_clear = false;
                self.foreground_shell_exit_reported = false;
                self.presence.observe(found)
            }
        }
    }

    fn current_title(&mut self, pane: &impl Pane) -> String {
        if self.title_writes_at_change == Some(pane.title_writes()) {
            return String::new();
        }
        self.title_writes_at_change = None;
        title(&pane.title())
    }

    /// Records what the rules concluded, returning it when it changed.
    fn conclude(&mut self, agent: Option<Agent>, state: State) -> Option<Publication> {
        let publication = Publication::concluded(agent, state);
        if self.last_concluded.as_ref() == Some(&publication) {
            return None;
        }
        self.last_concluded = Some(publication.clone());
        Some(publication)
    }

    fn emit(&mut self, publication: Publication) -> Option<Publication> {
        if self.last_emitted.as_ref() == Some(&publication) {
            return None;
        }
        self.last_emitted = Some(publication.clone());
        Some(publication)
    }
}

/// Where a [`Detector`] stands, as another process needs it to go on ([`Detector::carried`]).
/// What only this process could use - when it last looked, the screen text it last read - is
/// left out, and read again.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "the detector's own flags, one each")]
pub struct Carried {
    pub agent: Option<Agent>,
    pub misses: u8,
    pub state: State,
    pub visible: bool,
    pub emitted: Option<Publication>,
    pub grace_left: Option<Duration>,
    pub idle_seen_ago: Option<Duration>,
    pub idle_confirmations: u8,
    pub foreground_group: Option<u32>,
    pub probed: bool,
    pub shell_clear_pending: bool,
    pub shell_exit_reported: bool,
    pub title_pending: bool,
    /// What the screen rules last concluded.
    pub concluded: Option<Publication>,
    /// The agent's own report, and how long ago it came.
    pub report: Option<(Agent, State, Duration)>,
    /// How long ago the pane last produced output, which a working report goes stale from.
    pub output_ago: Option<Duration>,
}

/// What checking the foreground came to.
struct Foreground {
    group_changed: bool,
    agent_changed: bool,
    emitted: Option<Publication>,
}

/// The agent a pane is running, which survives a few probes that miss it: a probe can land
/// while the agent is between processes, and forgetting it then would publish the pane as
/// unknown and then idle again.
#[derive(Debug, Default)]
struct Presence {
    current: Option<Agent>,
    consecutive_misses: u8,
}

impl Presence {
    fn clear(&mut self) -> bool {
        self.consecutive_misses = 0;
        self.current.take().is_some()
    }

    /// Returns whether the agent changed.
    fn observe(&mut self, found: Option<Agent>) -> bool {
        if let Some(agent) = found {
            self.consecutive_misses = 0;
            if self.current.as_ref() == Some(&agent) {
                return false;
            }
            self.current = Some(agent);
            return true;
        }
        if self.current.is_none() {
            self.consecutive_misses = 0;
            return false;
        }
        self.consecutive_misses = self.consecutive_misses.saturating_add(1);
        if self.consecutive_misses < AGENT_MISS_CONFIRMATION_ATTEMPTS {
            return false;
        }
        self.clear()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundShellAgentAction {
    ObserveProbe,
    ReportProcessExit,
    ReportReplacementProcess,
    ClearAgent,
}

/// What a probe means for the agent the pane had. The pane's shell back in the foreground is
/// the agent exiting, but the agent is not forgotten at once: an idle for it is published
/// first, so whatever waits on it sees it finish before the pane becomes unknown.
fn foreground_shell_agent_action(
    previous: Option<&Agent>,
    found: Option<&Agent>,
    shell_in_foreground: bool,
    process_exit_reported: bool,
) -> ForegroundShellAgentAction {
    let Some(previous) = previous else {
        return ForegroundShellAgentAction::ObserveProbe;
    };
    if process_exit_reported {
        return if found == Some(previous) {
            ForegroundShellAgentAction::ReportReplacementProcess
        } else if found.is_none() {
            ForegroundShellAgentAction::ClearAgent
        } else {
            ForegroundShellAgentAction::ObserveProbe
        };
    }
    if found.is_some() {
        return ForegroundShellAgentAction::ObserveProbe;
    }
    if shell_in_foreground {
        return ForegroundShellAgentAction::ReportProcessExit;
    }
    ForegroundShellAgentAction::ObserveProbe
}

#[derive(Debug, Clone, Copy)]
struct ProbeInput {
    identified: bool,
    foreground_group: Option<u32>,
    last_foreground_group: Option<u32>,
    has_process_probe: bool,
    acquisition_age: Option<Duration>,
    pending_foreground_shell_clear: bool,
    elapsed_since_process_check: Duration,
}

fn foreground_group_changed(foreground_group: Option<u32>, last: Option<u32>) -> bool {
    foreground_group != last && (foreground_group.is_some() || last.is_some())
}

/// Only a group the terminal reported drives change detection. Remembering one the probe
/// inferred would read as a change on every tick while the terminal stays silent.
fn process_group_for_change_tracking(observed: Option<u32>, probed: Option<u32>) -> Option<u32> {
    observed?;
    probed.or(observed)
}

/// Probing reads several processes' arguments, so it happens when something suggests the
/// answer changed - the foreground group moved, a probe is owed, the screen started changing
/// in a pane with no agent yet - and otherwise only now and then, as a safety net.
fn should_probe_foreground_job(input: ProbeInput) -> bool {
    if input.pending_foreground_shell_clear {
        return true;
    }
    let group_changed =
        foreground_group_changed(input.foreground_group, input.last_foreground_group);

    if let Some(age) = input.acquisition_age {
        let interval = if age <= PROCESS_ACQUISITION_FAST_WINDOW {
            PROCESS_ACQUISITION_FAST_RECHECK
        } else {
            PROCESS_ACQUISITION_SLOW_RECHECK
        };
        if age <= PROCESS_ACQUISITION_WINDOW && input.elapsed_since_process_check >= interval {
            return true;
        }
    }

    if !input.identified {
        return !input.has_process_probe
            || group_changed
            || (input.foreground_group.is_none()
                && input.elapsed_since_process_check >= PROCESS_RECHECK_MISSING_FOREGROUND_GROUP);
    }
    group_changed || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

/// Output in a pane with no agent yet opens a window of faster probes: a wrapper that starts
/// its agent a moment later, in the same group, changes no group to notice.
fn sync_content_change_acquisition(
    identified: bool,
    group_changed: bool,
    content_changed: bool,
    now: Instant,
    acquisition_started_at: &mut Option<Instant>,
    last_content_change_at: &mut Option<Instant>,
) {
    if identified || group_changed {
        return;
    }
    if content_changed {
        let should_start = acquisition_started_at.is_none_or(|started| {
            now.saturating_duration_since(started) > PROCESS_ACQUISITION_WINDOW
                && last_content_change_at.is_none_or(|last| {
                    now.saturating_duration_since(last) >= PROCESS_ACQUISITION_IDLE_RESET
                })
        });
        if should_start {
            *acquisition_started_at = Some(now);
        }
        *last_content_change_at = Some(now);
        return;
    }
    let (Some(started), Some(last_change)) = (*acquisition_started_at, *last_content_change_at)
    else {
        return;
    };
    if now.saturating_duration_since(started) > PROCESS_ACQUISITION_WINDOW
        && now.saturating_duration_since(last_change) >= PROCESS_ACQUISITION_IDLE_RESET
    {
        *acquisition_started_at = None;
        *last_content_change_at = None;
    }
}

#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "herdr's inputs, kept so its tests port as they were"
)]
struct ScreenReadInput {
    state: State,
    identified: bool,
    pending_idle_active: bool,
    agent_changed: bool,
    process_exited: bool,
    content_seq: Option<u64>,
    last_screen_scan_content_seq: Option<u64>,
}

/// An idle agent whose output has not moved since its screen was last read has nothing new
/// to say, and reading a screen is the loop's one real cost.
fn should_skip_screen_read(input: ScreenReadInput) -> bool {
    if input.state != State::Idle
        || !input.identified
        || input.pending_idle_active
        || input.agent_changed
        || input.process_exited
    {
        return false;
    }
    input.content_seq.is_some() && input.last_screen_scan_content_seq == input.content_seq
}

/// What was last published, as the transition rules compare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PublishState {
    state: State,
    visible: bool,
}

/// Holds a working agent's plain idle - idle because no rule matched - until it has been seen
/// on three rechecks or has lasted 700 ms. An agent between two outputs looks idle for a frame.
#[derive(Debug, Default)]
struct PendingIdle {
    started_at: Option<Instant>,
    confirmations: u8,
}

impl PendingIdle {
    fn active(&self) -> bool {
        self.started_at.is_some()
    }

    fn clear(&mut self) {
        self.started_at = None;
        self.confirmations = 0;
    }

    fn should_hold_working_to_idle(
        &mut self,
        previous: PublishState,
        next: PublishState,
        agent_changed: bool,
        process_exited: bool,
        now: Instant,
    ) -> bool {
        let working_to_plain_idle = previous.state == State::Working
            && next.state == State::Idle
            && !next.visible
            && !agent_changed
            && !process_exited;
        if !working_to_plain_idle {
            self.clear();
            return false;
        }
        let Some(started_at) = self.started_at else {
            self.started_at = Some(now);
            self.confirmations = 0;
            return true;
        };
        if now.saturating_duration_since(started_at) >= PENDING_IDLE_CAP {
            self.clear();
            return false;
        }
        self.confirmations = self.confirmations.saturating_add(1);
        if self.confirmations >= PENDING_IDLE_CONFIRMATIONS {
            self.clear();
            return false;
        }
        true
    }
}

/// Whether `next` is published. herdr's rule is that a change of state or of its visibility
/// is, as is anything on an agent change or exit, unless it is a plain idle being held.
fn decide_transition(
    previous: PublishState,
    next: PublishState,
    agent_changed: bool,
    process_exited: bool,
    now: Instant,
    pending_idle: &mut PendingIdle,
) -> bool {
    if pending_idle.should_hold_working_to_idle(previous, next, agent_changed, process_exited, now)
    {
        return false;
    }
    next != previous || agent_changed || process_exited
}

#[cfg(test)]
mod tests;
