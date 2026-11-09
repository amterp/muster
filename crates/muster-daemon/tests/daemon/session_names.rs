//! A pane's name kept in step with its agent's session name (MIP-5, section 10). The agent is
//! the harness's fake, under `claude`'s manifest, whose `[session] rename` is `/rename {name}`:
//! it writes down every line it reads, so what the daemon typed into it is on disk. What the
//! harness calls its session reaches the daemon as a report, as Claude Code's statusline sends
//! it.

use std::path::PathBuf;
use std::time::Duration;

use crate::persistence::{record, stop, until_restored, until_saved};
use crate::reopen::{close, reopened};
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

    /// Whether the API says the pane's name came from its session, and what its record says.
    fn name_is_from_session(&mut self) -> bool {
        let record = record(&snapshot(&mut self.control), "p1").label_from_session;
        let mut api = self.daemon.api();
        let snapshot = api.subscribe(&[api::Topic::Panes]).snapshot().clone();
        assert_eq!(snapshot.panes[0].name_from_session, record);
        record
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

/// A client hides a name its harness may have written about a wake, so it asks where a name came
/// from. A person's rename settles that, even to the very text the pane took.
#[test]
fn a_name_taken_from_the_session_is_marked_until_somebody_renames_the_pane() {
    let mut p1 = Pane::named(None);
    assert!(!p1.name_is_from_session(), "no name is no mark");

    p1.reports("Read galaxy+ocean message");
    assert!(p1.name_is_from_session());
    p1.reports("Read galaxy+ocean message");
    assert!(p1.name_is_from_session(), "said again, it is the same name from the same place");

    p1.renamed("Read galaxy+ocean message");
    assert!(!p1.name_is_from_session(), "a rename adopts the text");
    assert_eq!(p1.label().as_deref(), Some("Read galaxy+ocean message"));

    p1.reports("another");
    assert!(p1.name_is_from_session(), "the session renamed again");
    let unnamed = proto::pane_request::Rename { pane: "p1".to_string(), label: None };
    let unnamed = pane(proto::pane_request::Request::Rename(unnamed));
    expect(&mut p1.control, unnamed, proto::Outcome::Done);
    assert!(!p1.name_is_from_session(), "no name is no mark");
}

/// A name given to the pane is handed to the session, which then says it back or says something
/// else first, and neither makes it the session's.
#[test]
fn a_given_name_is_not_marked_by_the_session_saying_it_or_starting_with_another() {
    let mut p1 = Pane::named(Some("the build"));
    p1.reports("Ok");
    assert_eq!(p1.label().as_deref(), Some("the build"));
    assert!(!p1.name_is_from_session());
    p1.reports("the build");
    assert!(!p1.name_is_from_session());

    p1.reports("reviewer");
    assert_eq!(p1.label().as_deref(), Some("reviewer"));
    assert!(p1.name_is_from_session(), "renamed in its harness, the pane took a name");
}

/// A restart that lost the mark would show a wake's title as a name somebody gave.
#[test]
fn the_mark_survives_a_restart_a_handoff_and_a_reopen() {
    let mut p1 = Pane::named(None);
    make(&mut p1.control, create("p2", beside("p1", proto::Side::Down)));
    p1.reports("a title");
    until_saved(&p1.daemon, "label_from_session");

    stop(&mut p1.daemon, &mut p1.control);
    p1.daemon.restart();
    p1.control = p1.daemon.connect();
    until_restored(&mut p1.control, 2);
    assert_eq!(p1.label().as_deref(), Some("a title"));
    assert!(p1.name_is_from_session(), "restarted");

    let answer = p1.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    p1.control = p1.daemon.connect();
    assert!(p1.name_is_from_session(), "handed over");

    close(&mut p1.control, "p1");
    reopened(&mut p1.control, "p1", "t9");
    assert!(p1.name_is_from_session(), "reopened");
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

/// The word a pane goes by until somebody names it is the daemon's, and not a name for the
/// session: typing it in would rename every agent's session after a word nobody chose, and the
/// session keeps whatever it is called.
#[test]
fn the_word_a_pane_was_drawn_is_never_typed_into_its_session() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), Vec::<String>::new());
    p1.reports("resumed");
    assert_eq!(p1.label().as_deref(), Some("resumed"), "the session's name still names the pane");
}

/// A session's name may be one another pane has, since given names may repeat, but not another
/// pane's callsign: the pane keeps the name it has rather than one it could never be reached by.
#[test]
fn a_session_name_another_panes_callsign_does_not_rename_the_pane() {
    let mut p1 = Pane::named(None);
    let p2 = proto::pane_request::Create {
        label: Some("critic".to_string()),
        ..create("p2", beside("p1", proto::Side::Right))
    };
    make(&mut p1.control, p2);
    let callsign = snapshot(&mut p1.control)
        .panes
        .into_iter()
        .find(|record| record.pane == "p2")
        .and_then(|record| record.callsign)
        .expect("p2 was drawn a callsign");
    p1.daemon.run_agent("p1");
    p1.reports("");
    p1.reports(&callsign);
    assert_eq!(p1.label(), None);
    p1.reports("critic");
    assert_eq!(p1.label().as_deref(), Some("critic"), "another pane's given name is free to share");
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

/// A rename left unsent in the prompt - Claude Code drops the Return of what is typed while it
/// starts - and the pane renamed again before it was seen taken: the first is sent, and then the
/// second, rather than the second waiting for good on a prompt the first still fills.
#[test]
fn a_pane_renamed_again_while_its_last_rename_sits_unsent_names_the_session_both_times() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_starting_agent("p1");
    p1.renamed("first");
    until_text(&mut p1.control, "p1", "PROBE-PROMPT> /rename first");
    p1.renamed("second");
    p1.until_renamed("/rename second");
    assert_eq!(p1.renames_heard(), ["/rename first", "/rename second"]);
}

/// A rename whose Return was not taken and whose agent went to work, as Codex took up a first
/// prompt with `/rename` still in its composer: it is given up then, and once the agent is idle
/// with the rename alone in its prompt, Return is pressed for it again, rather than leaving it
/// there to hold every later wake.
#[test]
fn a_rename_left_in_the_prompt_as_its_agent_went_to_work_is_sent_once_it_is_idle() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    p1.daemon.type_into("p1", "heedless");
    until_some("the agent to start discarding Returns", || {
        std::fs::read_to_string(p1.heard_file())
            .unwrap_or_default()
            .contains("heedless")
            .then_some(())
    });
    p1.renamed("cxp");
    until_text(&mut p1.control, "p1", "PROBE-PROMPT> /rename cxp");
    p1.daemon.unblock_agent_unasked();
    p1.daemon.until_agent("p1", proto::AgentState::Working);
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), Vec::<String>::new(), "a Return reached a heedless agent");

    p1.daemon.end_turn_unasked();
    p1.until_renamed("/rename cxp");
    assert_eq!(p1.renames_heard(), ["/rename cxp"], "typed again");
}

/// A rename whose every Return was swallowed while its agent stayed idle is given up, and is still
/// sent once the agent takes a Return again.
#[test]
fn a_rename_given_up_at_an_idle_prompt_is_sent_once_its_agent_takes_returns() {
    let mut p1 = Pane::named(None);
    p1.daemon.run_agent("p1");
    p1.daemon.type_into("p1", "heedless");
    until_some("the agent to start discarding Returns", || {
        std::fs::read_to_string(p1.heard_file())
            .unwrap_or_default()
            .contains("heedless")
            .then_some(())
    });
    let mut logging = p1.daemon.connect();
    let follow = proto::session_request::Request::FollowLog(proto::session_request::FollowLog {
        after: None,
    });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    p1.renamed("cxp");
    let given_up = r#""event":"session.name.given_up""#;
    let logged = logging.logged_until(given_up, Duration::from_mins(1));
    assert!(logged.iter().any(|line| line.line.contains(given_up)), "{logged:?}");

    let heeding = std::time::Instant::now();
    p1.daemon.end_turn_unasked();
    p1.until_renamed("/rename cxp");
    assert!(heeding.elapsed() < Duration::from_secs(20), "sent after {:?}", heeding.elapsed());
}

/// With `name_sessions = false` a pane's name is never typed into its agent's session, while a
/// session renamed in its harness still renames the pane, which types nothing.
#[test]
fn with_naming_off_nothing_is_typed_and_the_session_still_names_the_pane() {
    let mut p1 = Pane::named(None);
    let off =
        proto::session_request::Request::SetNameSessions(proto::SetNameSessions { name: false });
    expect(&mut p1.control, session(off), proto::Outcome::Done);
    p1.daemon.run_agent("p1");
    p1.renamed("builder");
    std::thread::sleep(WOULD_HAVE_TYPED);
    assert_eq!(p1.renames_heard(), Vec::<String>::new(), "typed with naming off");
    p1.reports("");
    p1.reports("critic");
    assert_eq!(p1.label().as_deref(), Some("critic"));

    let on =
        proto::session_request::Request::SetNameSessions(proto::SetNameSessions { name: true });
    expect(&mut p1.control, session(on), proto::Outcome::Done);
    p1.renamed("reviewer");
    p1.until_renamed("/rename reviewer");
}
