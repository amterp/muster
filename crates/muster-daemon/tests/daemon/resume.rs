//! A pane whose agent reported its session comes back after a restart running that session,
//! with the arguments the agent was started with less the ones that would start a session of
//! their own (`muster_detect::Resume`).

use crate::persistence::{record, stop, until_restored, until_saved};
use crate::support::*;
use proto::{pane_request, session_request};

/// The fake agent in `p1` of a fresh detecting daemon, started with `arguments`, its session
/// reported as `session` and saved.
fn reported(arguments: &str, session: &str) -> (Daemon, Control) {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent_with_arguments("p1", arguments);
    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some(session.to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    until_saved(&daemon, session);
    (daemon, control)
}

fn restarted(daemon: &mut Daemon, control: &mut Control) -> Control {
    stop(daemon, control);
    daemon.restart();
    let mut control = daemon.connect();
    until_restored(&mut control, 1);
    control
}

/// The flags go along, and the first prompt does not: it was sent when the session started.
#[test]
fn a_pane_comes_back_resuming_its_agents_session_with_its_flags() {
    let (mut daemon, mut control) =
        reported("--model opus --effort high 'the first prompt'", "s-1");

    let mut control = restarted(&mut daemon, &mut control);

    let command = until_some("p1 to come back running its agent", || {
        record(&snapshot(&mut control), "p1").command.clone()
    });
    assert!(
        command.ends_with("claude --model opus --effort high --resume s-1"),
        "resumed as {command:?}"
    );
    daemon.until_agent("p1", proto::AgentState::Idle);

    // The log says which flags went along and never their values: a carried flag can hold a
    // secret, and what somebody typed stays out of the run log.
    let log = std::fs::read_to_string(daemon.root().join("daemon.log")).unwrap_or_default();
    let resumed = log
        .lines()
        .find(|line| line.contains("daemon.state.resumed"))
        .unwrap_or_else(|| panic!("no daemon.state.resumed in the daemon's log:\n{log}"));
    assert!(resumed.contains("--model --effort --resume"), "{resumed}");
    assert!(!resumed.contains("opus") && !resumed.contains("high"), "{resumed}");
}

/// A word that could be a prompt or a flag's value carries nothing, rather than risk sending a
/// prompt twice: the session resumes with none of the arguments.
#[test]
fn arguments_that_cannot_be_told_from_a_prompt_are_not_carried() {
    let (mut daemon, mut control) = reported("--add-dir a b --model opus", "s-2");

    let mut control = restarted(&mut daemon, &mut control);

    let command = until_some("p1 to come back running its agent", || {
        record(&snapshot(&mut control), "p1").command.clone()
    });
    assert!(command.ends_with("claude --resume s-2"), "resumed as {command:?}");
}

/// With `resume_agents` off a pane comes back as a shell, as every pane did before.
#[test]
fn with_resuming_off_a_pane_comes_back_as_a_shell() {
    let (mut daemon, mut control) = reported("--model opus", "s-3");
    expect(
        &mut control,
        session(session_request::Request::SetResumeAgents(proto::SetResumeAgents {
            resume: false,
        })),
        proto::Outcome::Done,
    );
    until_saved(&daemon, "\"resume_agents\": false");

    let mut control = restarted(&mut daemon, &mut control);

    let after = snapshot(&mut control);
    assert_eq!(record(&after, "p1").command, None, "p1 came back running something");
}

/// A session resumed anywhere but where it ran finds no conversation, so a pane whose directory
/// is gone comes back as a shell rather than resuming from home.
#[test]
fn a_pane_whose_directory_is_gone_comes_back_as_a_shell() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    let gone = daemon.root().join("gone");
    std::fs::create_dir_all(&gone).expect("the test makes the directory");
    let made = pane_request::Create {
        cwd: Some(gone.display().to_string()),
        ..create("p1", in_new_tab("t1"))
    };
    make(&mut control, made);
    daemon.run_agent_with_arguments("p1", "--model opus");
    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some("s-4".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    until_saved(&daemon, "s-4");
    std::fs::remove_dir_all(&gone).expect("the test removes the directory");

    let mut control = restarted(&mut daemon, &mut control);

    let after = snapshot(&mut control);
    assert_eq!(record(&after, "p1").command, None, "p1 resumed from somewhere it never ran");
    let log = std::fs::read_to_string(daemon.root().join("daemon.log")).unwrap_or_default();
    assert!(log.contains("daemon.state.resume_skipped"), "{log}");
}
