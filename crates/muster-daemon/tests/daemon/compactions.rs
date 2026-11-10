//! An agent's context compacted at its prompt (MIP-5, section 10). The agent is the harness's
//! fake, under `claude`'s manifest, whose `[session] compact` is `/compact {focus}`: it writes
//! down every line it reads, so what the daemon typed into it is on disk. Its context reaches the
//! daemon as a report, as Claude Code's statusline sends it.

use std::path::PathBuf;
use std::time::Duration;

use crate::support::*;

/// Long enough for a compaction the daemon would type to have been typed: past the quiet period
/// after anything typed into the pane, and a look or two of the doorbell's thread.
const WOULD_HAVE_TYPED: Duration = Duration::from_secs(6);

struct Pane {
    daemon: Daemon,
    control: Control,
}

impl Pane {
    /// A daemon with the fake agent idle in pane `p1`.
    fn with_an_agent() -> Pane {
        let daemon = Daemon::start_detecting();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        daemon.run_agent("p1");
        Pane { daemon, control }
    }

    fn compact(&mut self, focus: Option<&str>) -> proto::Outcome {
        self.compact_then(focus, None).outcome()
    }

    /// Asks for `p1`'s agent to be compacted, keeping `focus`, and then given `then`.
    fn compact_then(&mut self, focus: Option<&str>, then: Option<&str>) -> Asked {
        let compact = proto::pane_request::Compact {
            pane: "p1".to_string(),
            focus: focus.map(str::to_string),
            then: then.map(str::to_string),
        };
        self.control.ask(pane(proto::pane_request::Request::Compact(compact)))
    }

    fn compact_at(&mut self, percent: Option<f32>) {
        let set = proto::session_request::Request::SetCompactAt(proto::SetCompactAt { percent });
        expect(&mut self.control, session(set), proto::Outcome::Done);
    }

    fn rules(&mut self, rules: proto::CompactionRules) {
        let set = proto::session_request::Request::SetCompactionRules(proto::SetCompactionRules {
            rules: Some(rules),
        });
        expect(&mut self.control, session(set), proto::Outcome::Done);
    }

    /// Sets `p1`'s own thresholds, each left as it is when none.
    fn own(&mut self, compact_at: Option<Option<u32>>, nudge_at: Option<Option<u32>>) {
        let set = proto::pane_request::SetCompaction {
            pane: "p1".to_string(),
            compact_at: compact_at.map(threshold),
            nudge_at: nudge_at.map(threshold),
            clear: false,
        };
        expect(
            &mut self.control,
            pane(proto::pane_request::Request::SetCompaction(set)),
            proto::Outcome::Done,
        );
    }

    fn record(&mut self) -> proto::Pane {
        let records = snapshot(&mut self.control).panes;
        records.into_iter().find(|record| record.pane == "p1").expect("p1 is open")
    }

    /// The percent `p1` is compacted at and what set it, then the same for its nudge.
    fn applied(&mut self) -> [(Option<u32>, proto::ThresholdSource); 2] {
        let applied = self.record().compaction.expect("the daemon says what applies");
        [applied.compact_at, applied.nudge_at].map(|threshold| {
            let threshold = threshold.unwrap_or_default();
            (threshold.percent, threshold.source())
        })
    }

    /// The wakes typed into the agent for messages from `@muster`, which is how a nudge
    /// reaches it, each once: a wake says which messages are new, so a message rung again, as a
    /// daemon rings what it took over unread, is the same line, and another message is not.
    fn nudges_heard(&self) -> Vec<String> {
        let mut heard: Vec<String> = Vec::new();
        for line in std::fs::read_to_string(self.heard_file()).unwrap_or_default().lines() {
            let rung = line.contains("[muster] ") && line.contains("from @muster");
            if rung && !heard.iter().any(|known| known == line) {
                heard.push(line.to_string());
            }
        }
        heard
    }

    fn until_nudged(&self, times: usize) {
        until(
            &format!("{times} nudges to reach the agent"),
            || self.nudges_heard().len() >= times,
            (),
        );
    }

    fn withheld(&mut self) -> proto::NudgeWithheld {
        self.record().compaction.unwrap_or_default().nudge_withheld()
    }

    fn queued(&mut self) -> bool {
        self.record().pending_compaction() == proto::PendingCompaction::Queued
    }

    /// How full the agent says its context is, as `claude`'s statusline would.
    fn reports(&mut self, context_used: f32) {
        let report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            context_used: Some(context_used),
            ..Default::default()
        };
        let asked = self.control.ask(pane(proto::pane_request::Request::Report(report)));
        assert!(
            matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo),
            "the report was refused: {}",
            asked.answer.reason
        );
    }

    /// [`Pane::reports`], asked again while the daemon refuses it. A daemon that took the pane
    /// over can take a report from this test for another agent's until it has placed the pane's
    /// own, and refuses it.
    fn reports_once_taken(&mut self, context_used: f32) {
        let control = &mut self.control;
        until_some("the daemon to take the agent's report", || {
            let report = proto::pane_request::Report {
                pane: "p1".to_string(),
                agent: "claude".to_string(),
                context_used: Some(context_used),
                ..Default::default()
            };
            let asked = control.ask(pane(proto::pane_request::Request::Report(report)));
            (asked.outcome() != proto::Outcome::Refused).then_some(())
        });
    }

    /// Reports `state` as `claude`'s hooks do, with `waiting` as the prompt hook sends one.
    fn hook_reports(&mut self, state: proto::AgentState, waiting: Option<&str>) {
        let mut report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            waiting: waiting.map(str::to_string),
            ..Default::default()
        };
        report.set_state(state);
        expect(
            &mut self.control,
            pane(proto::pane_request::Request::Report(report)),
            proto::Outcome::Done,
        );
    }

    /// Declares a wait, as the agent does before ending its turn to wait on its own work.
    fn declares(&mut self, waiting: &str) {
        let report = proto::pane_request::Report {
            pane: "p1".to_string(),
            waiting: Some(waiting.to_string()),
            ..Default::default()
        };
        expect(
            &mut self.control,
            pane(proto::pane_request::Request::Report(report)),
            proto::Outcome::Done,
        );
    }

    fn waiting_on(&mut self) -> Option<String> {
        let records = snapshot(&mut self.control).panes;
        let record = records.into_iter().find(|record| record.pane == "p1")?;
        record.facts.and_then(|facts| facts.waiting)
    }

    /// Has the daemon load its manifests again, as an app connecting does.
    fn send_manifests(&mut self) {
        let send = session(proto::session_request::Request::SendManifests(
            proto::SendManifests::default(),
        ));
        let asked = self.control.ask(send);
        assert!(
            matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo),
            "sending the manifests was refused: {}",
            asked.answer.reason
        );
    }

    /// Waits until `p1`'s record says whether its agent compacts, with the agent still claude.
    fn until_record_compacts(&mut self, compacts: bool) {
        let control = &mut self.control;
        until_some(&format!("p1's record to say its agent compacts: {compacts}"), || {
            let record = snapshot(control).panes.into_iter().find(|record| record.pane == "p1")?;
            (record.agent.as_deref() == Some("claude") && record.agent_compacts == compacts)
                .then_some(())
        });
    }

    fn heard_file(&self) -> PathBuf {
        self.daemon.root().join("home/fake-agent-heard")
    }

    /// The compactions the agent has read.
    fn compactions_heard(&self) -> Vec<String> {
        std::fs::read_to_string(self.heard_file())
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("/compact"))
            .map(str::to_string)
            .collect()
    }

    /// Every line the agent has read, in order.
    fn heard(&self) -> Vec<String> {
        std::fs::read_to_string(self.heard_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn until_heard_line(&self, line: &str) {
        until_some(&format!("the agent to read {line:?}"), || {
            self.heard().iter().any(|heard| heard == line).then_some(())
        });
    }

    /// Waits until `p1`'s record says what it has queued to type at its agent's prompt.
    fn until_pending(&mut self, pending: proto::PendingCompaction) {
        let control = &mut self.control;
        until_some(&format!("p1's record to say {pending:?}"), || {
            let record = snapshot(control).panes.into_iter().find(|record| record.pane == "p1")?;
            (record.pending_compaction() == pending).then_some(())
        });
    }

    fn until_heard(&self, count: usize) {
        until_some(&format!("the agent to read {count} compactions"), || {
            (self.compactions_heard().len() >= count).then_some(())
        });
    }
}

#[test]
fn a_compaction_asked_of_an_agent_at_work_is_typed_once_it_is_idle() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    assert_eq!(p1.compact(Some(" keep the parser notes ")), proto::Outcome::Done);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new(), "typed into an agent at work");

    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);
    p1.until_heard(1);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), ["/compact keep the parser notes"], "typed once");
}

#[test]
fn a_compaction_is_refused_where_nothing_could_type_it() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut shell = Pane { daemon, control };
    assert_eq!(shell.compact(None), proto::Outcome::Refused, "a shell has no context");

    let mut p1 = Pane::with_an_agent();
    assert_eq!(p1.compact(Some("keep\nthe notes")), proto::Outcome::Refused, "two lines");
    assert_eq!(p1.compact(Some(&"x".repeat(513))), proto::Outcome::Refused, "a brief");
    let elsewhere =
        proto::pane_request::Compact { pane: "p9".to_string(), focus: None, then: None };
    let asked = p1.control.ask(pane(proto::pane_request::Request::Compact(elsewhere)));
    assert_eq!(asked.outcome(), proto::Outcome::NotThere);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new());
}

fn threshold(percent: Option<u32>) -> proto::CompactionThreshold {
    proto::CompactionThreshold { percent }
}

fn thresholds(
    compact_at: Option<Option<u32>>,
    nudge_at: Option<Option<u32>>,
) -> Option<proto::CompactionThresholds> {
    Some(proto::CompactionThresholds {
        compact_at: compact_at.map(threshold),
        nudge_at: nudge_at.map(threshold),
    })
}

/// The most specific place that sets a threshold decides it: the pane, then the directory it is
/// in, then its harness, then the root. Read off the pane's record, with the agent at work so
/// that a compaction queued stays queued.
#[test]
fn a_pane_is_compacted_at_the_most_specific_threshold_set_for_it() {
    use proto::ThresholdSource as Source;
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    let unset = (None, Source::Unset);
    let root = thresholds(Some(Some(85)), None);
    let claude_exempt = proto::CompactionHarness {
        name: "claude".to_string(),
        thresholds: thresholds(Some(None), None),
    };

    p1.rules(proto::CompactionRules { root: root.clone(), ..Default::default() });
    assert_eq!(p1.applied(), [(Some(85), Source::Root), unset]);

    p1.rules(proto::CompactionRules {
        root: root.clone(),
        harnesses: vec![claude_exempt.clone()],
        ..Default::default()
    });
    assert_eq!(
        p1.applied(),
        [(None, Source::Harness), unset],
        "\"none\" for claude beats the root"
    );
    p1.reports(99.0);
    assert!(!p1.queued(), "claude is exempt");

    let here = p1.record().cwd;
    let around = std::path::Path::new(&here).parent().unwrap().display().to_string();
    let directories = vec![
        proto::CompactionDirectory { path: around, thresholds: thresholds(Some(Some(90)), None) },
        proto::CompactionDirectory { path: here, thresholds: thresholds(Some(Some(50)), None) },
    ];
    p1.rules(proto::CompactionRules { root, harnesses: vec![claude_exempt], directories });
    assert_eq!(p1.applied(), [(Some(50), Source::Directory), unset], "the longest directory");
    p1.reports(60.0);
    assert!(p1.queued(), "past the directory's 50");

    p1.own(Some(None), None);
    assert_eq!(p1.applied(), [(None, Source::Pane), unset]);
    assert!(!p1.queued(), "the threshold that queued it no longer applies");

    p1.own(Some(Some(70)), None);
    p1.reports(60.0);
    assert!(!p1.queued(), "60 is under the pane's own 70");
    p1.reports(75.0);
    assert!(p1.queued());

    let clear = proto::pane_request::SetCompaction {
        pane: "p1".to_string(),
        clear: true,
        ..Default::default()
    };
    expect(
        &mut p1.control,
        pane(proto::pane_request::Request::SetCompaction(clear)),
        proto::Outcome::Done,
    );
    assert_eq!(p1.applied(), [(Some(50), Source::Directory), unset], "the rules decide again");
    assert_eq!(p1.record().own_compaction, None);
}

/// A pane's own thresholds last as long as the pane does: a daemon that restarted, one that took
/// the pane over, and a reopening all bring them back.
#[test]
fn a_panes_own_thresholds_survive_a_restart_a_handoff_and_a_reopen() {
    use crate::persistence::{stop, until_restored, until_saved};
    use proto::ThresholdSource as Source;
    let mut p1 = Pane::with_an_agent();
    make(&mut p1.control, create("p2", beside("p1", proto::Side::Down)));
    p1.own(Some(None), Some(Some(70)));
    let own = thresholds(Some(None), Some(Some(70)));
    until_saved(&p1.daemon, "nudge_at");

    stop(&mut p1.daemon, &mut p1.control);
    p1.daemon.restart();
    p1.control = p1.daemon.connect();
    until_restored(&mut p1.control, 2);
    assert_eq!(p1.record().own_compaction, own, "restarted");
    assert_eq!(p1.applied(), [(None, Source::Pane), (Some(70), Source::Pane)], "and applied");

    let answer = p1.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    p1.control = p1.daemon.connect();
    assert_eq!(p1.record().own_compaction, own, "handed over");

    crate::reopen::close(&mut p1.control, "p1");
    crate::reopen::reopened(&mut p1.control, "p1", "t9");
    assert_eq!(p1.record().own_compaction, own, "reopened");
}

/// Past `nudge_at` the agent is posted a message from `@muster`, once each time its context
/// crosses the line.
#[test]
fn past_nudge_at_an_agent_is_told_to_compact_itself_once_per_crossing() {
    let mut p1 = Pane::with_an_agent();
    p1.rules(proto::CompactionRules {
        root: thresholds(Some(Some(85)), Some(Some(60))),
        ..Default::default()
    });
    p1.reports(50.0);
    p1.reports(62.0);
    p1.until_nudged(1);
    p1.reports(70.0);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.nudges_heard().len(), 1, "nudged again while still past it");
    assert_eq!(p1.compactions_heard(), Vec::<String>::new(), "70 is under compact_at");

    p1.reports(30.0);
    p1.reports(61.0);
    p1.until_nudged(2);
}

/// An agent that passes both thresholds in one turn is compacted and not also told to compact
/// itself.
#[test]
fn an_agent_about_to_be_compacted_is_not_nudged() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    p1.rules(proto::CompactionRules {
        root: thresholds(Some(Some(85)), Some(Some(60))),
        ..Default::default()
    });
    p1.reports(90.0);
    assert!(p1.queued());
    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);
    p1.until_heard(1);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.nudges_heard(), Vec::<String>::new());
}

/// A nudge that could do no good is not sent, and the pane's record says why: one at or above
/// the percent the pane is compacted at, and one for an agent whose harness gives no way to
/// compact it, as Codex's does not.
#[test]
fn a_nudge_that_would_do_no_good_is_withheld_and_the_record_says_why() {
    let mut p1 = Pane::with_an_agent();
    p1.rules(proto::CompactionRules {
        root: thresholds(Some(Some(85)), Some(Some(60))),
        ..Default::default()
    });
    assert_eq!(p1.withheld(), proto::NudgeWithheld::None);

    p1.own(None, Some(Some(85)));
    assert_eq!(p1.withheld(), proto::NudgeWithheld::AtOrAboveCompactAt);
    p1.reports(84.0);
    p1.own(None, Some(Some(60)));

    let override_file = p1.daemon.root().join("home/.muster/agent-detection/claude.toml");
    let without: String = std::fs::read_to_string(&override_file)
        .expect("the harness wrote it")
        .lines()
        .filter(|line| !line.trim_start().starts_with("compact"))
        .flat_map(|line| [line, "\n"])
        .collect();
    std::fs::write(&override_file, &without).expect("the override is rewritten");
    p1.send_manifests();
    p1.until_record_compacts(false);
    assert_eq!(p1.withheld(), proto::NudgeWithheld::HarnessCannotCompact);
    p1.reports(84.5);

    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.nudges_heard(), Vec::<String>::new());
}

/// A daemon that takes a pane over posts no second nudge for the crossing the old one nudged.
#[test]
fn a_handoff_carries_that_an_agent_was_nudged() {
    let mut p1 = Pane::with_an_agent();
    p1.rules(proto::CompactionRules {
        root: thresholds(None, Some(Some(60))),
        ..Default::default()
    });
    p1.reports(62.0);
    p1.until_nudged(1);

    let answer = p1.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    p1.control = p1.daemon.connect();
    p1.reports_once_taken(63.0);
    std::thread::sleep(WOULD_HAVE_TYPED);
    let heard = p1.nudges_heard();
    assert_eq!(heard.len(), 1, "nudged again for the same crossing: {heard:?}");

    p1.reports_once_taken(30.0);
    p1.reports_once_taken(64.0);
    p1.until_nudged(2);
}

/// A compaction somebody asked for is theirs: no change of threshold takes it back.
#[test]
fn a_change_of_threshold_leaves_a_compaction_somebody_asked_for() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    p1.compact_at(Some(50.0));
    p1.reports(60.0);
    assert_eq!(p1.compact(Some("the plan")), proto::Outcome::Done);
    p1.own(Some(None), None);
    assert!(p1.queued());
}

/// A client that knows only one percent for every agent replaces whatever rules were held.
#[test]
fn one_percent_for_every_agent_replaces_the_rules() {
    use proto::ThresholdSource as Source;
    let mut p1 = Pane::with_an_agent();
    p1.rules(proto::CompactionRules {
        root: thresholds(Some(Some(85)), Some(Some(60))),
        harnesses: vec![proto::CompactionHarness {
            name: "claude".to_string(),
            thresholds: thresholds(Some(None), None),
        }],
        ..Default::default()
    });
    p1.compact_at(Some(80.0));
    assert_eq!(p1.applied(), [(Some(80), Source::Root), (None, Source::Unset)]);

    let refused = p1.control.ask(pane(proto::pane_request::Request::SetCompaction(
        proto::pane_request::SetCompaction {
            pane: "p1".to_string(),
            nudge_at: Some(threshold(Some(101))),
            ..Default::default()
        },
    )));
    assert_eq!(refused.outcome(), proto::Outcome::Refused, "101 is no percent of a context");
}

#[test]
fn past_compact_at_an_agent_is_compacted_once_per_crossing() {
    let mut p1 = Pane::with_an_agent();
    p1.reports(99.0);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new(), "compacted with compact_at unset");

    p1.compact_at(Some(80.0));
    p1.reports(85.0);
    p1.until_heard(1);
    p1.reports(86.0);
    p1.reports(90.0);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), ["/compact"], "compacted again while still past it");

    p1.reports(30.0);
    p1.reports(82.0);
    p1.until_heard(2);
    assert_eq!(p1.compactions_heard(), ["/compact", "/compact"]);
}

/// A pane wanting a rename and a compaction gets both, the rename first, and neither is given up
/// for the other sitting in its prompt.
#[test]
fn a_rename_and_a_compaction_are_both_typed_in_turn() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    let rename =
        proto::pane_request::Rename { pane: "p1".to_string(), label: Some("builder".to_string()) };
    expect(
        &mut p1.control,
        pane(proto::pane_request::Request::Rename(rename)),
        proto::Outcome::Done,
    );
    assert_eq!(p1.compact(None), proto::Outcome::Done);
    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);

    p1.until_heard(1);
    std::thread::sleep(WOULD_HAVE_TYPED);
    let lines: Vec<String> = std::fs::read_to_string(p1.heard_file())
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with('/'))
        .map(str::to_string)
        .collect();
    assert_eq!(lines, ["/rename builder", "/compact"]);
}

#[test]
fn turning_compact_at_off_takes_back_its_compaction_not_yet_typed() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    p1.compact_at(Some(50.0));
    p1.reports(60.0);
    p1.compact_at(None);
    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new());
}

/// A view offers compacting on the record's word rather than a manifest of its own, so the
/// record says it of the agent this daemon detected, and of nothing else.
#[test]
fn a_panes_record_says_whether_its_agent_compacts() {
    let mut p1 = Pane::with_an_agent();
    make(&mut p1.control, create("p2", in_new_tab("t2")));

    let records = snapshot(&mut p1.control).panes;
    let compacts = |name: &str| {
        records.iter().find(|record| record.pane == name).map(|record| record.agent_compacts)
    };
    assert_eq!(compacts("p1"), Some(true), "claude's manifest gives `/compact`");
    assert_eq!(compacts("p2"), Some(false), "a shell has no agent to compact");
}

/// An override saved while an agent runs can give it a compact line or take one away, and
/// detection still finds the same agent, so nothing else would say it again. Sending the
/// manifests, which the app does on every connect, does.
#[test]
fn an_override_that_changes_the_compact_line_is_said_again() {
    let mut p1 = Pane::with_an_agent();
    let override_file = p1.daemon.root().join("home/.muster/agent-detection/claude.toml");
    let with_compact = std::fs::read_to_string(&override_file).expect("the harness wrote it");
    let without: String = with_compact
        .lines()
        .filter(|line| !line.trim_start().starts_with("compact"))
        .flat_map(|line| [line, "\n"])
        .collect();
    assert_ne!(without, with_compact, "the harness's manifest gives a compact line to remove");

    std::fs::write(&override_file, &without).expect("the override is rewritten");
    p1.send_manifests();
    p1.until_record_compacts(false);

    std::fs::write(&override_file, &with_compact).expect("the override is restored");
    p1.send_manifests();
    p1.until_record_compacts(true);
}

/// A compaction asked to be followed by a prompt has it typed as the agent's next prompt, once the
/// compaction has run: not while the agent is still compacting, which reads as working. The record
/// says which of the two is queued all the while.
#[test]
fn a_compaction_with_a_prompt_has_it_typed_once_the_compaction_has_run() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    let asked = p1.compact_then(Some("keep the work notes"), Some("carry on with the plan"));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    p1.until_pending(proto::PendingCompaction::Queued);

    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);
    p1.until_heard_line("/compact keep the work notes");
    p1.until_pending(proto::PendingCompaction::FollowUp);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert!(
        !p1.heard().iter().any(|line| line == "carry on with the plan"),
        "typed while the compaction ran: {:?}",
        p1.heard()
    );

    p1.daemon.type_into("p1", "rest");
    p1.until_heard_line("carry on with the plan");
    p1.until_pending(proto::PendingCompaction::None);
    let heard = p1.heard();
    let at = |line: &str| heard.iter().position(|heard| heard == line);
    assert!(at("/compact keep the work notes") < at("carry on with the plan"), "{heard:?}");
}

/// An agent that reports its own state reads idle for a moment after its screen has gone to work
/// on the compaction, since its last idle report outranks the screen until the screen has said
/// otherwise for a while. The compaction is taken all the same, and its prompt follows it.
#[test]
fn a_compaction_taken_while_the_agents_idle_report_still_stands_has_its_prompt_typed() {
    let mut p1 = Pane::with_an_agent();
    p1.daemon.type_into("p1", "hook --agent claude --state idle");
    let reported = p1.daemon.root().join("home/fake-agent-reported");
    until_some("the agent's idle report to be taken", || {
        std::fs::read_to_string(&reported).ok().filter(|said| said.contains("hook 0"))
    });
    let asked = p1.compact_then(Some("keep the work notes"), Some("carry on with the plan"));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);

    p1.until_heard_line("/compact keep the work notes");
    p1.until_pending(proto::PendingCompaction::FollowUp);
    p1.daemon.type_into("p1", "rest");
    p1.until_heard_line("carry on with the plan");
}

/// A wait the agent declared outlasts a compaction typed while it waits: the adapter's prompt
/// hook reports the compaction's turn with no wait, as it reports a person's prompt, and the turn
/// ends without the agent saying the wait again. A person's prompt still ends it.
#[test]
fn a_declared_wait_outlasts_a_compaction_typed_while_it_waits() {
    use proto::AgentState::{Idle, Working};
    let mut p1 = Pane::with_an_agent();
    p1.hook_reports(Working, None);
    p1.declares("the full gate");
    p1.hook_reports(Idle, None);
    assert_eq!(p1.waiting_on().as_deref(), Some("the full gate"));

    assert_eq!(p1.compact(Some("keep the work notes")), proto::Outcome::Done);
    p1.until_heard_line("/compact keep the work notes");
    p1.hook_reports(Working, Some(""));
    p1.hook_reports(Idle, None);
    assert_eq!(p1.waiting_on().as_deref(), Some("the full gate"), "the compaction ended it");

    p1.daemon.type_into("p1", "rest");
    p1.until_heard_line("rest");
    p1.hook_reports(Working, Some(""));
    assert_eq!(p1.waiting_on(), None, "a person's prompt");
}

/// The same with no hooks, where the turn's start read off the screen is all that says a prompt
/// came; a turn somebody starts still ends the wait.
#[test]
fn a_declared_wait_outlasts_a_compaction_typed_into_an_agent_with_no_hooks() {
    use proto::AgentState::{Idle, Working};
    let mut p1 = Pane::with_an_agent();
    p1.daemon.set_agent_state("p1", Working);
    p1.declares("the full gate");
    p1.daemon.set_agent_state("p1", Idle);

    assert_eq!(p1.compact(Some("keep the work notes")), proto::Outcome::Done);
    p1.until_heard_line("/compact keep the work notes");
    p1.daemon.until_agent("p1", Working);
    p1.daemon.type_into("p1", "rest");
    p1.daemon.until_agent("p1", Idle);
    assert_eq!(p1.waiting_on().as_deref(), Some("the full gate"), "the compaction ended it");

    p1.daemon.set_agent_state("p1", Working);
    p1.daemon.set_agent_state("p1", Idle);
    assert_eq!(p1.waiting_on(), None, "a turn somebody started");
}

/// A later ask is what its asker means: one with no prompt takes back a prompt still owed.
#[test]
fn a_later_compaction_takes_back_a_prompt_still_owed() {
    let mut p1 = Pane::with_an_agent();
    let asked = p1.compact_then(Some("work through it"), Some("carry on"));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    p1.until_pending(proto::PendingCompaction::FollowUp);

    assert_eq!(p1.compact(Some("again")), proto::Outcome::Done);
    p1.until_pending(proto::PendingCompaction::Queued);
    p1.daemon.type_into("p1", "rest");
    p1.until_heard_line("/compact again");
    p1.until_pending(proto::PendingCompaction::None);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert!(!p1.heard().iter().any(|line| line == "carry on"), "{:?}", p1.heard());
}

#[test]
fn a_prompt_to_carry_on_with_is_refused_as_a_focus_is() {
    let mut p1 = Pane::with_an_agent();
    let refused = |p1: &mut Pane, then: &str| p1.compact_then(None, Some(then)).outcome();
    assert_eq!(refused(&mut p1, "two\nlines"), proto::Outcome::Refused);
    assert_eq!(refused(&mut p1, &"x".repeat(513)), proto::Outcome::Refused);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new(), "a refused ask compacted");
}
