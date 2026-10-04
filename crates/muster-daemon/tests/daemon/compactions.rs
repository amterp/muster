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
        let compact = proto::pane_request::Compact {
            pane: "p1".to_string(),
            focus: focus.map(str::to_string),
        };
        self.control.ask(pane(proto::pane_request::Request::Compact(compact))).outcome()
    }

    fn compact_at(&mut self, percent: Option<f32>) {
        let set = proto::session_request::Request::SetCompactAt(proto::SetCompactAt { percent });
        expect(&mut self.control, session(set), proto::Outcome::Done);
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
    let elsewhere = proto::pane_request::Compact { pane: "p9".to_string(), focus: None };
    let asked = p1.control.ask(pane(proto::pane_request::Request::Compact(elsewhere)));
    assert_eq!(asked.outcome(), proto::Outcome::NotThere);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.compactions_heard(), Vec::<String>::new());
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
