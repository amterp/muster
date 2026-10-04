//! A pane's name kept in step with its agent's session name (MIP-5, section 10). The agent is
//! the harness's fake, under `claude`'s manifest, whose `[session] rename` is `/rename {name}`:
//! it writes down every line it reads, so what the daemon typed into it is on disk. What the
//! harness calls its session reaches the daemon as a report, as Claude Code's statusline sends
//! it.

use std::path::PathBuf;
use std::time::Duration;

use crate::support::*;

/// Long enough for a rename the daemon would type to have been typed: past the quiet period
/// after anything typed into the pane, and a look or two of the doorbell's thread.
const WOULD_HAVE_TYPED: Duration = Duration::from_secs(6);

struct Pane {
    daemon: Daemon,
    control: Control,
}

impl Pane {
    /// A daemon with pane `p1`, named `label` when given, at its shell.
    fn named(label: Option<&str>) -> Pane {
        let daemon = Daemon::start_detecting();
        let mut control = daemon.connect();
        let create = proto::pane_request::Create {
            label: label.map(str::to_string),
            ..create("p1", in_new_tab("t1"))
        };
        make(&mut control, create);
        Pane { daemon, control }
    }

    fn renamed(&mut self, label: &str) {
        let rename =
            proto::pane_request::Rename { pane: "p1".to_string(), label: Some(label.to_string()) };
        expect(
            &mut self.control,
            pane(proto::pane_request::Request::Rename(rename)),
            proto::Outcome::Done,
        );
    }

    /// What the harness says its session is called, as `claude`'s statusline would.
    fn reports(&mut self, session_name: &str) {
        let report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            session_name: Some(session_name.to_string()),
            ..Default::default()
        };
        let asked = self.control.ask(pane(proto::pane_request::Request::Report(report)));
        assert!(
            matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo),
            "the report was refused: {}",
            asked.answer.reason
        );
    }

    fn label(&mut self) -> Option<String> {
        snapshot(&mut self.control)
            .panes
            .into_iter()
            .find(|record| record.pane == "p1")
            .and_then(|record| record.label)
    }

    fn heard_file(&self) -> PathBuf {
        self.daemon.root().join("home/fake-agent-heard")
    }

    /// The renames the agent has read.
    fn renames_heard(&self) -> Vec<String> {
        std::fs::read_to_string(self.heard_file())
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("/rename"))
            .map(str::to_string)
            .collect()
    }

    fn until_renamed(&self, line: &str) {
        until_some(&format!("the agent to read {line:?}"), || {
            self.renames_heard().iter().any(|heard| heard == line).then_some(())
        });
    }
}

#[test]
fn a_pane_renamed_names_its_agents_session_once_the_agent_is_idle() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    p1.daemon.set_agent_state("p1", proto::AgentState::Working);
    p1.renamed("🤖 A");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), Vec::<String>::new(), "typed into an agent at work");

    p1.daemon.set_agent_state("p1", proto::AgentState::Idle);
    p1.until_renamed("/rename 🤖 A");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), ["/rename 🤖 A"], "typed once");
}

#[test]
fn a_pane_named_when_it_was_made_names_the_session_of_the_agent_started_in_it() {
    let p1 = Pane::named(Some("builder"));
    p1.daemon.run_agent("p1");
    p1.until_renamed("/rename builder");
}

#[test]
fn a_session_renamed_in_its_harness_renames_the_pane_and_nothing_is_typed_back() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    p1.reports("");
    p1.reports("critic");
    assert_eq!(p1.label().as_deref(), Some("critic"));
    p1.reports("critic");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), Vec::<String>::new());

    // A person renaming the pane while the statusline still says the old name.
    p1.renamed("reviewer");
    p1.reports("critic");
    assert_eq!(p1.label().as_deref(), Some("reviewer"), "the old name, said again, is not news");
    p1.until_renamed("/rename reviewer");
}

#[test]
fn the_name_a_session_starts_with_gives_way_to_the_name_its_pane_already_has() {
    let mut p1 = Pane::named(Some("lexer"));
    // Before detection has found the agent, as a statusline can report.
    p1.reports("resumed session");
    assert_eq!(p1.label().as_deref(), Some("lexer"));
    p1.daemon.run_agent("p1");
    p1.until_renamed("/rename lexer");
    p1.reports("lexer");
    p1.reports("parser");
    assert_eq!(p1.label().as_deref(), Some("parser"), "a rename after the session started");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), ["/rename lexer"]);
}

#[test]
fn a_session_name_needs_its_agent_and_counts_only_from_the_agent_in_the_pane() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    let unnamed = proto::pane_request::Report {
        pane: "p1".to_string(),
        session_name: Some("x".to_string()),
        ..Default::default()
    };
    expect(
        &mut p1.control,
        pane(proto::pane_request::Request::Report(unnamed)),
        proto::Outcome::Refused,
    );
    let other = proto::pane_request::Report {
        pane: "p1".to_string(),
        agent: "codex".to_string(),
        session_name: Some("x".to_string()),
        ..Default::default()
    };
    p1.control.ask(pane(proto::pane_request::Request::Report(other)));
    assert_eq!(p1.label(), None);
}

/// A session's name too long to keep is not a reason to lose the rest of the report: the
/// statusline sends the context, model and cost beside it, and a refusal would drop them all.
#[test]
fn a_session_name_too_long_to_keep_leaves_the_rest_of_its_report() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    let report = proto::pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_name: Some("x".repeat(200)),
        context_used: Some(42.0),
        ..Default::default()
    };
    expect(
        &mut p1.control,
        pane(proto::pane_request::Request::Report(report)),
        proto::Outcome::Done,
    );
    let record =
        snapshot(&mut p1.control).panes.into_iter().find(|record| record.pane == "p1").unwrap();
    assert_eq!(record.facts.and_then(|facts| facts.context_used), Some(42.0));
    assert_eq!(record.label, None, "the name was not taken");
}
