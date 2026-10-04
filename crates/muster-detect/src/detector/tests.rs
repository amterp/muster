//! The first half is herdr v0.8.0's own tests of these decisions, from `src/pane.rs` and
//! `src/pane/agent_detection.rs` (Apache-2.0), minus those of what was not ported. The second
//! drives whole ticks over a pane and processes of the test's making, with time passed in.

// The subtractions are of constants a millisecond apart, or of an instant from a later one.
#![allow(clippy::unchecked_time_subtraction)]

use super::*;
use crate::process::{Job, Process};

fn publish_state(state: State) -> PublishState {
    PublishState { state, visible: false }
}

fn claude() -> Agent {
    Agent::new("claude")
}

// ---- herdr's decision tests ----

#[test]
fn foreground_shell_reports_process_exit_before_clearing_agent() {
    let codex = Agent::new("codex");
    assert_eq!(
        foreground_shell_agent_action(Some(&codex), None, true, false),
        ForegroundShellAgentAction::ReportProcessExit
    );
    assert_eq!(
        foreground_shell_agent_action(Some(&codex), None, true, true),
        ForegroundShellAgentAction::ClearAgent
    );
}

#[test]
fn same_agent_after_reported_exit_is_a_replacement_process() {
    let pi = Agent::new("pi");
    assert_eq!(
        foreground_shell_agent_action(Some(&pi), Some(&pi), false, true),
        ForegroundShellAgentAction::ReportReplacementProcess
    );
}

#[test]
fn unknown_non_shell_foreground_job_is_not_immediate_clear_signal() {
    assert_eq!(
        foreground_shell_agent_action(Some(&claude()), None, false, false),
        ForegroundShellAgentAction::ObserveProbe
    );
}

#[test]
fn reported_process_exit_clears_before_unknown_foreground_probe() {
    assert_eq!(
        foreground_shell_agent_action(Some(&claude()), None, false, true),
        ForegroundShellAgentAction::ClearAgent
    );
}

#[test]
fn foreground_agent_job_is_not_clear_signal() {
    assert_eq!(
        foreground_shell_agent_action(Some(&claude()), Some(&Agent::new("opencode")), true, false),
        ForegroundShellAgentAction::ObserveProbe
    );
}

fn probe_input() -> ProbeInput {
    ProbeInput {
        identified: false,
        foreground_group: Some(42),
        last_foreground_group: Some(42),
        has_process_probe: true,
        acquisition_age: None,
        pending_foreground_shell_clear: false,
        elapsed_since_process_check: Duration::from_secs(1),
        changed_since_process_check: false,
    }
}

#[test]
fn unchanged_unidentified_foreground_group_skips_full_process_probe() {
    assert!(!should_probe_foreground_job(probe_input()));
}

#[test]
fn unidentified_foreground_group_change_runs_full_process_probe() {
    assert!(should_probe_foreground_job(ProbeInput {
        foreground_group: Some(43),
        ..probe_input()
    }));
}

#[test]
fn unidentified_pane_gets_initial_process_probe() {
    assert!(should_probe_foreground_job(ProbeInput { has_process_probe: false, ..probe_input() }));
}

#[test]
fn stable_unidentified_foreground_group_has_no_safety_process_probe() {
    assert!(!should_probe_foreground_job(ProbeInput {
        elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
        ..probe_input()
    }));
}

#[test]
fn unidentified_pane_without_foreground_group_uses_safety_process_probe() {
    let no_group =
        ProbeInput { foreground_group: None, last_foreground_group: None, ..probe_input() };
    assert!(!should_probe_foreground_job(no_group));
    assert!(should_probe_foreground_job(ProbeInput {
        elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
        ..no_group
    }));
}

#[test]
fn unidentified_pane_probes_when_foreground_group_disappears() {
    assert!(should_probe_foreground_job(ProbeInput {
        foreground_group: None,
        last_foreground_group: Some(42),
        ..probe_input()
    }));
}

#[test]
fn inferred_group_does_not_trigger_a_probe_on_every_tick() {
    let tracked = process_group_for_change_tracking(None, Some(300));
    assert_eq!(tracked, None);
    assert!(!should_probe_foreground_job(ProbeInput {
        identified: true,
        foreground_group: None,
        last_foreground_group: tracked,
        elapsed_since_process_check: Duration::from_millis(300),
        ..probe_input()
    }));
}

#[test]
fn pending_shell_clear_forces_a_process_probe() {
    assert!(should_probe_foreground_job(ProbeInput {
        identified: true,
        pending_foreground_shell_clear: true,
        ..probe_input()
    }));
}

#[test]
fn acquisition_window_catches_delayed_same_group_wrapper_startup() {
    let acquiring = |age, elapsed| {
        should_probe_foreground_job(ProbeInput {
            acquisition_age: Some(age),
            elapsed_since_process_check: elapsed,
            ..probe_input()
        })
    };
    let fast = Duration::from_millis(1250);
    assert!(!acquiring(fast, PROCESS_ACQUISITION_FAST_RECHECK - Duration::from_millis(1)));
    assert!(acquiring(fast, PROCESS_ACQUISITION_FAST_RECHECK));
    assert!(acquiring(Duration::from_secs(5), PROCESS_ACQUISITION_SLOW_RECHECK));
    assert!(!acquiring(
        PROCESS_ACQUISITION_WINDOW + Duration::from_millis(1),
        PROCESS_ACQUISITION_SLOW_RECHECK
    ));
}

#[test]
fn content_change_starts_bounded_unidentified_acquisition_window() {
    let now = Instant::now();
    let (mut started, mut last_change) = (None, None);
    sync_content_change_acquisition(false, false, true, now, &mut started, &mut last_change);
    assert_eq!((started, last_change), (Some(now), Some(now)));

    let later = now + Duration::from_secs(1);
    sync_content_change_acquisition(false, false, true, later, &mut started, &mut last_change);
    assert_eq!(started, Some(now), "changed frames should not refresh the acquisition window");
    assert_eq!(last_change, Some(later));

    let quiet = later + PROCESS_ACQUISITION_WINDOW + PROCESS_ACQUISITION_IDLE_RESET;
    sync_content_change_acquisition(false, false, false, quiet, &mut started, &mut last_change);
    assert_eq!((started, last_change), (None, None));

    let burst = quiet + Duration::from_secs(1);
    sync_content_change_acquisition(false, false, true, burst, &mut started, &mut last_change);
    assert_eq!((started, last_change), (Some(burst), Some(burst)));
}

#[test]
fn content_change_does_not_start_acquisition_when_process_probe_has_other_signal() {
    let now = Instant::now();
    for (identified, group_changed) in [(true, false), (false, true)] {
        let (mut started, mut last_change) = (None, None);
        sync_content_change_acquisition(
            identified,
            group_changed,
            true,
            now,
            &mut started,
            &mut last_change,
        );
        assert_eq!((started, last_change), (None, None));
    }
}

#[test]
fn content_change_restarts_stale_process_group_acquisition_window() {
    let now = Instant::now() + PROCESS_ACQUISITION_WINDOW * 2;
    let mut started = Some(now - PROCESS_ACQUISITION_WINDOW - Duration::from_millis(1));
    let mut last_change = None;
    sync_content_change_acquisition(false, false, true, now, &mut started, &mut last_change);
    assert_eq!((started, last_change), (Some(now), Some(now)));
}

#[test]
fn identified_agent_uses_shorter_safety_process_probe() {
    let identified = |elapsed| {
        should_probe_foreground_job(ProbeInput {
            identified: true,
            elapsed_since_process_check: elapsed,
            ..probe_input()
        })
    };
    assert!(!identified(PROCESS_RECHECK_IDENTIFIED - Duration::from_millis(1)));
    assert!(identified(PROCESS_RECHECK_IDENTIFIED));
}

#[test]
fn identified_agent_probes_when_foreground_group_disappears() {
    assert!(should_probe_foreground_job(ProbeInput {
        identified: true,
        foreground_group: None,
        last_foreground_group: Some(42),
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED - Duration::from_millis(1),
        ..probe_input()
    }));
}

#[test]
fn stable_missing_foreground_group_uses_safety_process_probe() {
    let no_group = |elapsed| {
        should_probe_foreground_job(ProbeInput {
            identified: true,
            foreground_group: None,
            last_foreground_group: None,
            elapsed_since_process_check: elapsed,
            ..probe_input()
        })
    };
    assert!(!no_group(PROCESS_RECHECK_IDENTIFIED - Duration::from_millis(1)));
    assert!(no_group(PROCESS_RECHECK_IDENTIFIED));
}

#[test]
fn transient_process_miss_keeps_current_agent_detected() {
    let mut presence = Presence { current: Some(Agent::new("pi")), consecutive_misses: 0 };
    assert!(!presence.observe(None), "one miss should not clear the detected agent");
    assert_eq!(presence.current, Some(Agent::new("pi")));
}

#[test]
fn agent_only_clears_after_confirmation_misses() {
    let mut presence = Presence { current: Some(Agent::new("pi")), consecutive_misses: 0 };
    for attempt in 1..AGENT_MISS_CONFIRMATION_ATTEMPTS {
        assert!(!presence.observe(None), "miss {attempt} should stay in the confirmation window");
        assert_eq!(presence.current, Some(Agent::new("pi")));
    }
    assert!(presence.observe(None), "the last confirmation miss should clear the agent");
    assert_eq!(presence.current, None);
}

fn screen_read(state: State, content_seq: u64) -> ScreenReadInput {
    ScreenReadInput {
        state,
        identified: true,
        pending_idle_active: false,
        agent_changed: false,
        process_exited: false,
        content_seq: Some(content_seq),
        last_screen_scan_content_seq: Some(10),
    }
}

#[test]
fn screen_read_skips_unchanged_idle_bottom_buffer() {
    assert!(should_skip_screen_read(screen_read(State::Idle, 10)));
}

#[test]
fn screen_read_reads_when_idle_bottom_buffer_changes() {
    assert!(!should_skip_screen_read(screen_read(State::Idle, 11)));
}

#[test]
fn screen_read_reads_for_transitions_and_missing_agent() {
    for input in [
        ScreenReadInput { pending_idle_active: true, ..screen_read(State::Idle, 10) },
        ScreenReadInput { agent_changed: true, ..screen_read(State::Idle, 10) },
        ScreenReadInput { process_exited: true, ..screen_read(State::Idle, 10) },
        ScreenReadInput { identified: false, ..screen_read(State::Idle, 10) },
        screen_read(State::Working, 10),
    ] {
        assert!(!should_skip_screen_read(input), "{input:?}");
    }
}

#[test]
fn pending_idle_holds_working_to_plain_idle_until_confirmed() {
    let now = Instant::now();
    let (previous, next) = (publish_state(State::Working), publish_state(State::Idle));
    let mut pending = PendingIdle::default();
    for recheck in 0..3 {
        assert!(pending.should_hold_working_to_idle(
            previous,
            next,
            false,
            false,
            now + PENDING_IDLE_RECHECK * recheck
        ));
    }
    assert!(!pending.should_hold_working_to_idle(
        previous,
        next,
        false,
        false,
        now + PENDING_IDLE_RECHECK * 3
    ));
}

#[test]
fn pending_idle_is_released_by_its_cap_whatever_the_count() {
    let now = Instant::now();
    let (previous, next) = (publish_state(State::Working), publish_state(State::Idle));
    let mut pending = PendingIdle::default();
    assert!(pending.should_hold_working_to_idle(previous, next, false, false, now));
    assert!(!pending.should_hold_working_to_idle(
        previous,
        next,
        false,
        false,
        now + PENDING_IDLE_CAP
    ));
}

#[test]
fn visible_idle_bypasses_plain_idle_hold() {
    let mut pending = PendingIdle::default();
    let next = PublishState { state: State::Idle, visible: true };
    assert!(!pending.should_hold_working_to_idle(
        publish_state(State::Working),
        next,
        false,
        false,
        Instant::now()
    ));
}

#[test]
fn transition_decision_publishes_next_for_visible_blocker() {
    let blocked = PublishState { state: State::Blocked, visible: true };
    assert!(decide_transition(
        publish_state(State::Idle),
        blocked,
        false,
        false,
        Instant::now(),
        &mut PendingIdle::default()
    ));
}

#[test]
fn nothing_new_is_not_published() {
    assert!(!decide_transition(
        publish_state(State::Working),
        publish_state(State::Working),
        false,
        false,
        Instant::now(),
        &mut PendingIdle::default()
    ));
}

// ---- whole ticks ----

const SHELL: u32 = 100;
const AGENT_GROUP: u32 = 200;

#[derive(Debug, Default)]
struct FakePane {
    group: Option<u32>,
    content_seq: u64,
    input_at: Option<Instant>,
    screen: String,
    title: String,
    title_writes: u64,
    progress: String,
    screen_reads: usize,
}

impl FakePane {
    fn set_title(&mut self, title: &str) {
        self.title = title.to_string();
        self.title_writes += 1;
    }
}

impl Pane for FakePane {
    fn foreground_group(&self) -> Option<u32> {
        self.group
    }
    fn content_seq(&self) -> u64 {
        self.content_seq
    }
    fn input_at(&self) -> Option<Instant> {
        self.input_at
    }
    fn screen_text(&mut self) -> String {
        self.screen_reads += 1;
        self.screen.clone()
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn title_writes(&self) -> u64 {
        self.title_writes
    }
    fn progress(&self) -> String {
        self.progress.clone()
    }
    fn clear_progress(&mut self) {
        self.progress.clear();
    }
}

/// The shell's group holds the shell; the agent's holds whatever `agent` names.
#[derive(Debug)]
struct FakeProcesses {
    agent: &'static str,
}

impl FakeProcesses {
    fn job(&self, group: u32) -> Option<Job> {
        let (pid, name) = match group {
            SHELL => (SHELL, "zsh"),
            AGENT_GROUP => (AGENT_GROUP, self.agent),
            _ => return None,
        };
        Some(Job {
            group,
            processes: vec![Process { pid, name: name.into(), argv0: None, argv: None }],
        })
    }
}

impl Processes for FakeProcesses {
    fn leader(&self, group: u32) -> Option<Job> {
        self.job(group)
    }
    fn job(&self, _shell: u32, group: u32) -> Option<Job> {
        FakeProcesses::job(self, group)
    }
    fn agent_hint(&self, _pid: u32) -> Option<String> {
        None
    }
}

/// Claude's rules replaced by markers a test can paint.
const MANIFEST: &str = r#"
id = "claude"
version = "9999.1"
min_engine_version = 1

[[rules]]
id = "title_working"
state = "working"
priority = 20
region = "osc_title"
visible_working = true
regex = ['^[\x{2800}-\x{28FF}] ']

[[rules]]
id = "busy"
state = "working"
priority = 10
contains = ["busy"]

[[rules]]
id = "prompt"
state = "idle"
priority = 10
visible_idle = true
contains = ["ready>"]

[[rules]]
id = "ask"
state = "blocked"
priority = 30
visible_blocker = true
contains = ["allow?"]

[[rules]]
id = "viewer"
state = "unknown"
priority = 40
skip_state_update = true
contains = ["transcript"]
"#;

struct Run {
    detector: Detector,
    pane: FakePane,
    processes: FakeProcesses,
    manifests: Manifests,
    now: Instant,
    next: Duration,
}

impl Run {
    /// A pane whose shell is at its prompt.
    fn new() -> Run {
        Run::with_manifest(MANIFEST)
    }

    fn with_manifest(manifest: &str) -> Run {
        let (manifests, warnings) =
            Manifests::load(&[("claude.toml".into(), manifest.into())], None);
        assert_eq!(warnings, []);
        let now = Instant::now();
        Run {
            detector: Detector::new(SHELL, now),
            pane: FakePane { group: Some(SHELL), ..FakePane::default() },
            processes: FakeProcesses { agent: "claude" },
            manifests,
            now,
            next: Detector::FIRST_TICK,
        }
    }

    fn tick(&mut self) -> Option<Publication> {
        self.now += self.next;
        let tick = self.detector.tick(self.now, &mut self.pane, &self.processes, &self.manifests);
        self.next = tick.next;
        tick.publication
    }

    /// Ticks until something is published, or the time runs out.
    fn until_published(&mut self, within: Duration) -> Option<(Duration, Publication)> {
        let start = self.now;
        while self.now - start < within {
            if let Some(publication) = self.tick() {
                return Some((self.now - start, publication));
            }
        }
        None
    }

    fn paint(&mut self, screen: &str) {
        self.pane.screen = screen.to_string();
        self.pane.content_seq += 1;
    }

    /// Starts claude in the pane and waits out its grace.
    fn start_agent(&mut self) {
        self.pane.group = Some(AGENT_GROUP);
        assert_eq!(self.tick(), Some(published(Some(claude()), State::Idle)));
        self.settle();
    }

    /// Ticks through the startup grace, asserting nothing is published during it.
    fn settle(&mut self) {
        let start = self.now;
        while self.detector.startup_grace_until.is_some() {
            assert_eq!(self.tick(), None, "nothing is published during the grace");
        }
        assert!(self.now - start >= STARTUP_GRACE);
    }
}

fn published(agent: Option<Agent>, state: State) -> Publication {
    Publication { agent, state, reported: false, unreadable: false }
}

#[test]
fn a_pane_with_no_agent_is_unknown_and_ticks_slowly() {
    let mut run = Run::new();
    assert_eq!(run.tick(), Some(published(None, State::Unknown)));
    assert_eq!(run.next, TICK_UNIDENTIFIED);
    assert_eq!(run.tick(), None);
}

#[test]
fn a_new_agent_is_idle_at_once_and_its_screen_waits_out_the_grace() {
    let mut run = Run::new();
    run.tick();
    run.paint("busy");
    run.pane.group = Some(AGENT_GROUP);

    assert_eq!(run.tick(), Some(published(Some(claude()), State::Idle)));
    assert_eq!(run.next, TICK_IDENTIFIED);
    let (after, publication) = run.until_published(Duration::from_secs(10)).unwrap();
    assert_eq!(publication, published(Some(claude()), State::Working));
    assert!(after > STARTUP_GRACE, "working was published {after:?} in, inside the grace");
}

#[test]
fn working_to_a_plain_idle_is_held_until_it_has_been_seen_four_times() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    assert_eq!(run.until_published(Duration::from_secs(1)).unwrap().1.state, State::Working);

    run.paint("nothing matches this");
    let held_from = run.now;
    let mut reads = 0;
    let publication = loop {
        let publication = run.tick();
        reads += 1;
        if publication.is_some() {
            break publication;
        }
        assert_eq!(run.next, PENDING_IDLE_RECHECK, "a held idle is rechecked quickly");
    };
    assert_eq!(publication, Some(published(Some(claude()), State::Idle)));
    assert_eq!(reads, 4, "seen once, then confirmed three times");
    assert!(run.now - held_from < PENDING_IDLE_CAP);
}

#[test]
fn a_visible_idle_or_a_blocker_is_published_at_once() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(1)).unwrap();

    run.paint("ready>");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Idle)));
    run.paint("allow?");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Blocked)));
}

#[test]
fn a_skip_rule_freezes_the_state() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(1)).unwrap();

    run.paint("transcript: ready> allow? busy");
    assert_eq!(run.until_published(Duration::from_secs(2)), None);
}

#[test]
fn an_idle_pane_whose_output_has_not_moved_is_not_read_again() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.tick();
    let reads = run.pane.screen_reads;
    for _ in 0..10 {
        run.tick();
    }
    assert_eq!(run.pane.screen_reads, reads, "an unchanged idle screen was read again");
    run.paint("busy");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Working)));
    assert_eq!(run.pane.screen_reads, reads + 1);
}

#[test]
fn returning_to_the_shell_publishes_idle_and_then_unknown() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(1)).unwrap();

    run.pane.group = Some(SHELL);
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Idle)), "the agent finished");
    assert_eq!(run.tick(), Some(published(None, State::Unknown)), "and then it is gone");
    assert_eq!(run.detector.agent(), None);
}

#[test]
fn an_agent_is_forgotten_only_after_six_probes_miss_it() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    // The agent's group is still in the foreground, but the probe cannot name it.
    run.processes.agent = "mystery";
    let mut probes = 0;
    while run.detector.agent().is_some() {
        run.now += PROCESS_RECHECK_IDENTIFIED;
        run.tick();
        probes += 1;
        assert!(probes <= 6, "the agent outlived six misses");
    }
    assert_eq!(probes, 6);
}

#[test]
fn a_new_agent_does_not_inherit_the_last_title_or_progress() {
    let mut run = Run::new();
    run.tick();
    run.pane.set_title("⠂ left over");
    run.pane.progress = "4;1;50".to_string();
    run.start_agent();
    assert_eq!(run.pane.progress, "", "progress was cleared on the agent change");

    run.paint("nothing");
    assert_eq!(run.until_published(Duration::from_secs(1)), None, "the stale title counted");

    run.pane.set_title("⠄ fresh");
    run.paint("nothing still");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Working)));
}

#[test]
fn reset_forgets_the_agent_and_finds_it_again() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("allow?");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Blocked)));

    run.detector.reset();
    assert_eq!(run.detector.agent(), None);
    run.next = Duration::ZERO;
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Idle)));
    run.settle();
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Blocked)));
}

#[test]
fn a_reset_publishes_the_truth_even_when_the_agent_went_with_its_manifest() {
    const SPRITE: &str = r#"
id = "sprite"
version = "2026.01.01.1"
min_engine_version = 1

[[rules]]
id = "busy"
state = "working"
priority = 10
contains = ["busy"]
"#;
    let mut run = Run::new();
    let (manifests, warnings) = Manifests::load(&[("sprite.toml".into(), SPRITE.into())], None);
    assert_eq!(warnings, []);
    run.manifests = manifests;
    run.processes.agent = "sprite";
    run.tick();
    run.pane.group = Some(AGENT_GROUP);
    run.tick();
    run.settle();
    run.paint("busy");
    let sprite = Agent::new("sprite");
    assert_eq!(run.tick(), Some(published(Some(sprite), State::Working)));

    // The only manifest that knew sprite is gone: nothing names it now.
    run.manifests = Manifests::built_in();
    run.detector.reset();
    run.next = Duration::ZERO;
    assert_eq!(run.tick(), Some(published(None, State::Unknown)));
    assert_eq!(run.until_published(Duration::from_secs(2)), None, "said once");
}

#[test]
fn a_title_written_again_after_an_agent_change_counts_though_it_is_the_same() {
    let mut run = Run::new();
    run.tick();
    run.pane.set_title("⠂ working on it");
    run.start_agent();
    run.paint("nothing");
    assert_eq!(run.until_published(Duration::from_secs(1)), None, "the old agent's title");

    // A restarted agent writing the title its predecessor left, word for word.
    run.pane.set_title("⠂ working on it");
    run.paint("nothing still");
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Working)));
}

/// A detector resumed from where another stood stands there too, its timers run on by however
/// long passed since.
#[test]
fn a_resumed_detector_goes_on_from_where_it_was_carried() {
    let carried = Carried {
        agent: Some(claude()),
        misses: 2,
        state: State::Working,
        visible: true,
        emitted: Some(published(Some(claude()), State::Working)),
        grace_left: Some(Duration::from_millis(1500)),
        idle_seen_ago: Some(Duration::from_millis(200)),
        idle_confirmations: 1,
        foreground_group: Some(4242),
        probed: true,
        shell_clear_pending: false,
        shell_exit_reported: false,
        title_pending: true,
        concluded: Some(published(Some(claude()), State::Idle)),
        report: Some(CarriedReport {
            agent: claude(),
            state: State::Working,
            ago: Duration::from_millis(500),
            confirmed: true,
        }),
        output_ago: Some(Duration::from_millis(50)),
        drift: Drift {
            rules_idle_ago: Some(Duration::from_secs(40)),
            unmatched_ago: None,
            working_ago: Some(Duration::from_secs(30)),
            active_ago: vec![Duration::from_secs(2), Duration::from_secs(1)],
        },
        blocker_ago: Some(Duration::from_secs(3)),
    };
    let now = Instant::now();
    let detector = Detector::resumed(100, carried.clone(), now, 7);
    assert_eq!(detector.agent(), Some(&claude()));
    assert_eq!(detector.title_writes_at_change, Some(7));

    let later = detector.carried(now + Duration::from_millis(100));
    assert_eq!(
        later,
        Carried {
            grace_left: Some(Duration::from_millis(1400)),
            idle_seen_ago: Some(Duration::from_millis(300)),
            report: Some(CarriedReport {
                ago: Duration::from_millis(600),
                ..carried.report.clone().expect("a report")
            }),
            output_ago: Some(Duration::from_millis(150)),
            drift: Drift {
                rules_idle_ago: Some(Duration::from_millis(40_100)),
                unmatched_ago: None,
                working_ago: Some(Duration::from_millis(30_100)),
                active_ago: vec![Duration::from_millis(2100), Duration::from_millis(1100)],
            },
            blocker_ago: Some(Duration::from_millis(3100)),
            ..carried
        }
    );
}

// ---- the agent's own reports ----

fn reported(state: State) -> Publication {
    Publication { agent: Some(claude()), state, reported: true, unreadable: false }
}

impl Run {
    fn report(&mut self, state: State) {
        self.detector.report(claude(), state, self.now);
    }

    /// Ticks for `span`, painting a new screen each tick when `moving`, and returns everything
    /// published.
    fn run_for(&mut self, span: Duration, moving: Option<&str>) -> Vec<Publication> {
        let start = self.now;
        let mut published = Vec::new();
        let mut frame = 0;
        while self.now - start < span {
            if let Some(screen) = moving {
                frame += 1;
                self.paint(&format!("{screen} {frame}"));
            }
            published.extend(self.tick());
        }
        published
    }
}

#[test]
fn an_agents_own_report_outranks_the_rules() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("ready>");
    run.until_published(Duration::from_secs(2));

    run.report(State::Working);
    assert_eq!(run.tick(), Some(reported(State::Working)));
    run.report(State::Blocked);
    assert_eq!(run.tick(), Some(reported(State::Blocked)));
    run.report(State::Idle);
    assert_eq!(run.tick(), Some(reported(State::Idle)));
}

#[test]
fn a_report_during_the_grace_is_published_at_once() {
    let mut run = Run::new();
    run.tick();
    run.pane.group = Some(AGENT_GROUP);
    assert_eq!(run.tick(), Some(published(Some(claude()), State::Idle)));
    run.report(State::Working);
    assert_eq!(run.tick(), Some(reported(State::Working)));
}

#[test]
fn a_working_report_goes_stale_once_the_screen_stops_moving() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("ready>");
    run.until_published(Duration::from_secs(2));
    run.report(State::Working);
    run.tick();

    assert_eq!(run.run_for(Duration::from_secs(20), Some("thinking")), []);
    let quiet = run.run_for(reporting::QUIET + Duration::from_secs(1), None);
    assert_eq!(quiet.last(), Some(&published(Some(claude()), State::Idle)), "{quiet:?}");
}

#[test]
fn a_blocked_or_idle_report_holds_however_still_the_screen() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(2));
    for state in [State::Blocked, State::Idle] {
        run.report(state);
        assert_eq!(run.tick(), Some(reported(state)));
        assert_eq!(run.run_for(Duration::from_secs(30), None), []);
    }
}

/// A permission prompt approved: nothing reports until the tool finishes, and the tool can run
/// for minutes with the screen moving. A report of waiting on you stops counting then, and the
/// rules read what the screen shows.
#[test]
fn a_blocked_report_yields_to_a_screen_that_keeps_moving() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("allow?");
    run.until_published(Duration::from_secs(2));
    run.report(State::Blocked);
    assert_eq!(run.tick(), Some(reported(State::Blocked)));

    let running = run.run_for(Duration::from_mins(4), Some("busy"));
    let last = running.last().expect("something was published");
    assert_eq!((last.state, last.reported), (State::Working, false), "{running:?}");
    assert!(
        running
            .iter()
            .all(|publication| !publication.reported || publication.state != State::Blocked),
        "{running:?}"
    );
}

/// Esc at a permission prompt, or a denial, runs no hook: the prompt goes, Claude Code is back
/// at its own, and the screen is still from then on. A blocked report the rules confirmed gives
/// way once they read something else.
#[test]
fn a_blocked_report_gives_way_once_the_prompt_it_described_is_gone() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(2));
    run.report(State::Blocked);
    run.paint("allow?");
    assert_eq!(run.tick(), Some(reported(State::Blocked)));

    run.paint("ready>");
    let after = run.run_for(Duration::from_secs(5), None);
    let last = after.last().expect("something was published");
    assert_eq!((last.state, last.reported), (State::Idle, false), "{after:?}");
}

/// Esc at a prompt, and the pane handed to another daemon before the report has given way: the
/// daemon it goes to knows the rules read the prompt, and lets the report go as this one would.
#[test]
fn a_blocked_report_gives_way_after_a_handoff_as_it_would_have_before() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(2));
    run.report(State::Blocked);
    run.paint("allow?");
    assert_eq!(run.tick(), Some(reported(State::Blocked)));
    run.paint("ready>");
    run.run_for(Duration::from_secs(1), None);

    run.detector = Detector::resumed(SHELL, run.detector.carried(run.now), run.now, 0);
    let after = run.run_for(Duration::from_secs(5), None);
    let last = after.last().expect("something was published");
    assert_eq!((last.state, last.reported), (State::Idle, false), "{after:?}");
}

/// A sibling's working report that lands just after the prompt draws: the prompt was already on
/// screen when it came, so the pane never reads working in between.
#[test]
fn a_working_report_just_after_a_prompt_draws_leaves_the_pane_blocked() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.run_for(Duration::from_secs(2), Some("busy"));
    run.report(State::Blocked);
    let mut seen = run.run_for(Duration::from_millis(500), Some("allow? busy"));
    run.report(State::Working);
    seen.extend(run.run_for(Duration::from_secs(5), Some("allow? busy")));
    assert!(seen.iter().all(|publication| publication.state == State::Blocked), "{seen:?}");
}

/// The same across a handoff: the daemon a pane goes to knows how long the prompt has been up.
#[test]
fn a_prompt_outranks_a_working_report_straight_through_a_handoff() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.run_for(Duration::from_secs(2), Some("busy"));
    run.report(State::Blocked);
    run.run_for(Duration::from_secs(3), Some("allow? busy"));
    run.report(State::Working);
    let mut seen = run.run_for(Duration::from_secs(1), Some("allow? busy"));

    run.detector = Detector::resumed(SHELL, run.detector.carried(run.now), run.now, 0);
    seen.extend(run.run_for(Duration::from_secs(5), Some("allow? busy")));
    assert!(seen.iter().all(|publication| publication.state == State::Blocked), "{seen:?}");
}

/// A prompt the rules read as blocked, with something else animating beside it: the report and
/// the rules agree, and the screen moving is no reason to stop counting it.
#[test]
fn a_blocked_report_the_rules_agree_with_holds_while_the_screen_moves() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("busy");
    run.until_published(Duration::from_secs(2));
    run.report(State::Blocked);
    run.paint("allow?");
    assert_eq!(run.tick(), Some(reported(State::Blocked)));

    let waiting = run.run_for(Duration::from_mins(4), Some("allow? busy"));
    assert_eq!(waiting, [], "the report stopped counting");
}

/// A hook can run before its prompt is drawn, while the rules still read the turn's spinner.
/// The prompt drawn a moment later confirms the report rather than the spinner ending it.
#[test]
fn a_blocked_report_that_comes_before_its_prompt_is_drawn_holds() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.run_for(Duration::from_secs(2), Some("busy"));
    run.report(State::Blocked);
    run.run_for(Duration::from_millis(300), Some("busy"));
    run.paint("allow?");
    let waiting = run.run_for(Duration::from_secs(30), None);
    assert!(waiting.iter().all(|publication| publication.reported), "{waiting:?}");
    assert_eq!(run.detector.effective(run.now), Some(reported(State::Blocked)));
}

/// Two sub-agents at once: one asks permission, and the other goes on calling tools, each of
/// which reports working when it ends, while their timer keeps the screen moving. The prompt
/// is still waiting on you, and the pane says so; once it closes, the working report counts
/// again.
#[test]
fn a_prompt_on_screen_outranks_a_working_report_from_beside_it() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.run_for(Duration::from_secs(2), Some("busy"));
    run.report(State::Blocked);
    run.run_for(Duration::from_secs(3), Some("allow? busy"));
    assert_eq!(run.detector.effective(run.now), Some(reported(State::Blocked)));

    let mut seen = Vec::new();
    for _ in 0..60 {
        run.report(State::Working);
        seen.extend(run.run_for(Duration::from_secs(1), Some("allow? busy")));
        assert_eq!(
            run.detector.effective(run.now).map(|publication| publication.state),
            Some(State::Blocked),
            "{seen:?}"
        );
    }
    assert!(seen.iter().all(|publication| publication.state == State::Blocked), "{seen:?}");

    let approved = run.run_for(Duration::from_secs(3), Some("busy"));
    assert_eq!(approved.last(), Some(&reported(State::Working)), "{approved:?}");
}

/// A report the rules never read the same way stops counting after output in each of three
/// seconds running, and output every 1.8 s, which leaves some seconds out, is not that.
#[test]
fn an_unconfirmed_report_lapses_on_output_in_every_second_and_not_less() {
    let lapses = |every: Duration| {
        let mut run = Run::new();
        run.tick();
        run.start_agent();
        run.paint("nothing a rule knows");
        run.until_published(Duration::from_secs(2));
        run.report(State::Blocked);
        assert_eq!(run.tick(), Some(reported(State::Blocked)));
        let start = run.now;
        let mut painted = start;
        let mut frame = 0;
        let mut published = Vec::new();
        while run.now - start < Duration::from_secs(8) {
            if run.now - painted >= every {
                painted = run.now;
                frame += 1;
                run.paint(&format!("still nothing {frame}"));
            }
            published.extend(run.tick());
        }
        published.iter().any(|publication| !publication.reported)
    };
    assert!(lapses(Duration::from_millis(1200)), "output every 1.2 s is output every second");
    assert!(!lapses(Duration::from_millis(1800)), "output every 1.8 s leaves seconds out");
}

/// An idle report while a background task keeps the screen moving is the rules' to read.
#[test]
fn an_idle_report_yields_to_a_screen_that_keeps_moving() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("ready>");
    run.until_published(Duration::from_secs(2));
    run.report(State::Idle);
    assert_eq!(run.tick(), Some(reported(State::Idle)));

    let running = run.run_for(Duration::from_secs(10), Some("busy"));
    let last = running.last().expect("something was published");
    assert_eq!((last.state, last.reported), (State::Working, false), "{running:?}");
}

/// A detector resumed from one that recorded no conclusion of the rules, as a daemon from before
/// they were recorded hands over, still lays a report over what was last published rather than
/// holding every report back until the rules see a change.
#[test]
fn a_resumed_report_counts_without_a_recorded_conclusion() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    let mut carried = run.detector.carried(run.now);
    carried.concluded = None;
    carried.report = Some(CarriedReport {
        agent: claude(),
        state: State::Blocked,
        ago: Duration::ZERO,
        confirmed: false,
    });
    run.detector = Detector::resumed(SHELL, carried, run.now, 0);
    // What the next tick lays the report over, before the rules conclude anything.
    let effective = run.detector.effective(run.now).expect("a report with nothing concluded");
    assert_eq!(effective, reported(State::Blocked));
    assert_eq!(run.tick(), Some(reported(State::Blocked)));
}

#[test]
fn a_report_stops_counting_when_the_agent_leaves() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.report(State::Working);
    assert_eq!(run.tick(), Some(reported(State::Working)));

    run.pane.group = Some(SHELL);
    let left = run.run_for(Duration::from_secs(3), None);
    assert!(left.iter().all(|publication| !publication.reported), "{left:?}");
    run.pane.group = Some(AGENT_GROUP);
    let back = run.run_for(Duration::from_secs(5), None);
    assert!(back.iter().all(|publication| !publication.reported), "{back:?}");
}

#[test]
fn a_report_before_the_agent_is_found_waits_briefly_for_it() {
    let mut run = Run::new();
    run.tick();
    run.report(State::Working);
    run.tick();
    run.pane.group = Some(AGENT_GROUP);
    let found = run.run_for(Duration::from_millis(900), None);
    assert!(found.contains(&reported(State::Working)), "{found:?}");

    let mut run = Run::new();
    run.tick();
    run.report(State::Working);
    run.run_for(reporting::UNCONFIRMED + Duration::from_millis(500), None);
    run.pane.group = Some(AGENT_GROUP);
    let late = run.run_for(Duration::from_secs(1), None);
    assert!(late.iter().all(|publication| !publication.reported), "{late:?}");
}

#[test]
fn a_moving_screen_no_rule_reads_is_said_to_be_unreadable() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    let published = run.run_for(reporting::DRIFT + Duration::from_secs(2), Some("spinning"));
    assert!(published.last().is_some_and(|last| last.unreadable), "{published:?}");

    let read = run.run_for(Duration::from_secs(1), Some("ready>"));
    assert!(read.last().is_some_and(|last| !last.unreadable), "a rule reads it again: {read:?}");
}

#[test]
fn rules_reading_idle_while_the_agent_reports_working_are_unreadable() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.report(State::Working);
    let published = run.run_for(reporting::DRIFT + Duration::from_secs(2), Some("ready>"));
    let last = published.last().expect("something was published");
    assert_eq!((last.state, last.reported, last.unreadable), (State::Working, true, true));
}

/// A manifest that says nothing of idle, as 11 of the built-in ones do.
const NO_IDLE_RULE: &str = r#"
id = "claude"
version = "9999.1"
min_engine_version = 1

[[rules]]
id = "busy"
state = "working"
priority = 10
contains = ["busy"]
"#;

/// A harness whose manifest leaves idle to the fallback, reworded so no rule reads it while it
/// works: its screen keeps moving and nothing matches, which is what drift exists to say.
#[test]
fn a_moving_screen_is_unreadable_for_a_manifest_with_no_idle_rule() {
    let mut run = Run::with_manifest(NO_IDLE_RULE);
    run.tick();
    run.start_agent();
    let published = run.run_for(reporting::DRIFT + Duration::from_secs(2), Some("⣾ reworded"));
    assert!(published.last().is_some_and(|last| last.unreadable), "{published:?}");
}

/// Many harnesses' manifests say nothing of idle and leave it to the fallback, so a screen no
/// rule matches is how they read idle. Typing a long prompt into one keeps its screen moving,
/// and that is not the rules failing to read it.
#[test]
fn typing_into_an_agent_whose_manifest_has_no_idle_rule_is_not_unreadable() {
    let mut run = Run::with_manifest(NO_IDLE_RULE);
    run.tick();
    run.start_agent();
    let start = run.now;
    let mut typed = Vec::new();
    let mut prompt = String::from("> ");
    while run.now - start < reporting::DRIFT + Duration::from_secs(2) {
        prompt.push('a');
        run.pane.input_at = Some(run.now);
        run.paint(&prompt);
        typed.extend(run.tick());
    }
    assert!(typed.iter().all(|publication| !publication.unreadable), "{typed:?}");
}

/// A pane whose screen the rules cannot read is still unreadable on the daemon it is handed to,
/// from its first tick: learning it again there would clear it for a minute, and say it again.
#[test]
fn unreadable_stays_so_across_a_handoff() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    let published = run.run_for(reporting::DRIFT + Duration::from_secs(2), Some("spinning"));
    assert!(published.last().is_some_and(|last| last.unreadable), "{published:?}");

    let carried = run.detector.carried(run.now);
    run.detector = Detector::resumed(SHELL, carried, run.now, 0);
    let after = run.run_for(Duration::from_secs(5), Some("still spinning"));
    assert!(after.iter().all(|publication| publication.unreadable), "{after:?}");
}

#[test]
fn a_still_screen_is_never_unreadable() {
    let mut run = Run::new();
    run.tick();
    run.start_agent();
    run.paint("nothing a rule knows");
    let published = run.run_for(reporting::DRIFT * 2, None);
    assert!(published.iter().all(|publication| !publication.unreadable), "{published:?}");
}

/// Under load a job's exec can land late in the window its group change opened: after the last
/// probe the window makes, with the agent's first paint still inside it. That paint cannot open a
/// window of its own while this one is open, and once it closes nothing probes a pane whose group
/// holds still - so the agent is never found.
#[test]
fn an_agent_that_starts_after_the_windows_last_probe_is_still_found() {
    let mut run = Run::new();
    run.tick();
    // The shell has forked the job into a group of its own and not yet exec'd it.
    run.processes.agent = "zsh";
    run.pane.group = Some(AGENT_GROUP);
    run.tick();
    let window_opened = run.now;
    while run.now - window_opened < Duration::from_millis(7500) {
        assert_eq!(run.tick(), None);
    }

    run.processes.agent = "claude";
    run.paint("ready>");

    let found = run.until_published(Duration::from_secs(30));
    assert_eq!(found.map(|(_, publication)| publication.agent), Some(Some(claude())));
}
