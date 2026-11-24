//! Restarting a pane's agent where it runs (`pane.restart`): its input emptied, its program
//! ended, and its session's resume typed at the shell it leaves behind.

use std::path::PathBuf;

use crate::persistence::{record, until_saved};
use crate::support::*;
use proto::pane_request;

/// The fake agent in `p1` of a fresh detecting daemon, started with `arguments`, its session
/// reported as `session`.
pub(crate) fn reported(
    environment: &[(&str, &str)],
    arguments: &str,
    session: &str,
) -> (Daemon, Control) {
    let daemon = Daemon::start_detecting_with(environment);
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

fn restart(name: &str, env: &[(&str, &str)]) -> proto::request::Service {
    let env = env.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string()));
    pane(pane_request::Request::Restart(pane_request::Restart {
        pane: name.to_string(),
        env: env.collect(),
    }))
}

pub(crate) fn restarted(asked: &Asked) -> proto::Restarted {
    match &asked.answer.detail {
        Some(proto::answer::Detail::Restarted(restarted)) => restarted.clone(),
        other => panic!("a restart answered with {other:?}"),
    }
}

fn home(daemon: &Daemon, file: &str) -> PathBuf {
    daemon.root().join("home").join(file)
}

/// What a file the fake agent writes as it starts says, once a new agent has written it.
fn written_again(daemon: &Daemon, file: &str, before: &str) -> String {
    until_some(&format!("a new agent to write {file}"), || {
        std::fs::read_to_string(home(daemon, file)).ok().filter(|now| now != before)
    })
}

/// The agent is ended and comes back in its session, with the flags it ran with, from the
/// command a daemon restart would run, and the answer says what was typed.
#[test]
fn an_agent_comes_back_in_its_session_with_its_flags() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-1");
    let pid = std::fs::read_to_string(home(&daemon, "fake-agent-pid")).expect("a pid");

    let asked = expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    let restarted = restarted(&asked);
    assert!(restarted.typed.ends_with("--model' 'opus' '--resume' 's-1'"), "{restarted:?}");
    assert!(!restarted.killed, "{restarted:?}");
    assert_ne!(written_again(&daemon, "fake-agent-pid", &pid), pid);
    assert_eq!(
        std::fs::read_to_string(home(&daemon, "fake-agent-args")).expect("its arguments").trim(),
        "--model opus --resume s-1"
    );
    daemon.until_agent("p1", proto::AgentState::Idle);
}

/// A changed environment reaches the resumed agent, and its value stays out of the run log.
#[test]
fn a_changed_environment_reaches_the_resumed_agent_alone() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-2");
    let before = std::fs::read_to_string(home(&daemon, "fake-agent-env")).expect("its env");

    expect(
        &mut control,
        restart("p1", &[("HTTPS_PROXY", "http://user:secret@proxy:3128")]),
        proto::Outcome::Done,
    );

    let env = written_again(&daemon, "fake-agent-env", &before);
    assert!(env.lines().any(|line| line == "HTTPS_PROXY=http://user:secret@proxy:3128"), "{env}");
    // Read once, straight after the answer, the log has been seen empty on a loaded machine.
    let path = daemon.root().join("daemon.log");
    until(
        "the restart in the daemon's log",
        || written(&path).contains("daemon.pane.restarting"),
        (),
    );
    let log = written(&path);
    assert!(log.contains("HTTPS_PROXY") && !log.contains("secret"), "{log}");
}

/// A restart's variables stay with the pane's resume, so a daemon restart afterwards brings the
/// agent back with them. Their values go to the state file, which only the user can read, and
/// never to the run log.
#[test]
fn a_restarts_environment_survives_a_daemon_restart() {
    let (mut daemon, mut control) = reported(&[], "--model opus", "s-3");
    let before = std::fs::read_to_string(home(&daemon, "fake-agent-env")).expect("its env");
    expect(
        &mut control,
        restart("p1", &[("HTTPS_PROXY", "http://user:secret@proxy:3128")]),
        proto::Outcome::Done,
    );
    let restarted_env = written_again(&daemon, "fake-agent-env", &before);
    daemon.until_agent("p1", proto::AgentState::Idle);
    // The resumed agent's hooks report its session as it starts, as Claude Code's do.
    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some("s-3".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    until_saved(&daemon, "HTTPS_PROXY");

    let mut control = crate::resume::restarted(&mut daemon, &mut control);
    let env = written_again(&daemon, "fake-agent-env", &restarted_env);
    assert!(env.lines().any(|line| line == "HTTPS_PROXY=http://user:secret@proxy:3128"), "{env}");
    daemon.until_agent("p1", proto::AgentState::Idle);
    let p1 = record(&snapshot(&mut control), "p1").clone();
    let resume = format!("{:?}", p1.resume);
    assert!(!resume.contains("secret"), "the record carries no value: {resume}");
    let log = written(&daemon.root().join("daemon.log"));
    assert!(!log.contains("secret"), "{log}");
}

/// A program that has stopped reading leaves what was typed at it in its terminal, where its
/// shell would read it as a command the moment the program ends. A restart empties it first.
#[test]
fn what_a_stopped_program_left_unread_never_reaches_its_shell() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-3");
    daemon.type_into("p1", "deaf 60");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut input = muster_harness::Input::connect(daemon.socket_path());
    input.send(
        "p1",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: "echo $((6*7))RAN".to_string(),
            enter: true,
            ..Default::default()
        }),
    );

    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    daemon.until_agent("p1", proto::AgentState::Idle);
    let screen = screen_text(&mut control, "p1");
    assert!(!screen.contains("42RAN"), "the shell ran what the program left unread: {screen}");
    let heard = std::fs::read_to_string(home(&daemon, "fake-agent-heard")).unwrap_or_default();
    assert!(!heard.contains("RAN"), "the resumed agent read it: {heard}");
}

/// A program that does not end when asked is killed once its grace is out.
#[test]
fn an_agent_that_ignores_being_asked_to_end_is_killed() {
    let (daemon, mut control) =
        reported(&[("MUSTER_DAEMON_RESTART_GRACE_MS", "500")], "--model opus", "s-4");
    daemon.type_into("p1", "stubborn");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let pid = std::fs::read_to_string(home(&daemon, "fake-agent-pid")).expect("a pid");

    let asked = expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    assert!(restarted(&asked).killed, "{:?}", restarted(&asked));
    assert_ne!(written_again(&daemon, "fake-agent-pid", &pid), pid);
}

/// An agent that reported no session has nothing to resume, and is left running; so is one
/// asked to start with a variable Muster sets on every pane.
#[test]
fn an_agent_with_nothing_to_resume_or_a_variable_muster_sets_is_left_running() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let asked = expect(&mut control, restart("p1", &[]), proto::Outcome::Refused);
    assert!(asked.answer.reason.contains("nothing to resume"), "{}", asked.answer.reason);
    expect(&mut control, restart("p9", &[]), proto::Outcome::NotThere);

    let (daemon, mut control) = reported(&[], "--model opus", "s-5");
    let asked =
        expect(&mut control, restart("p1", &[("MUSTER_PANE", "p2")]), proto::Outcome::Refused);
    assert!(asked.answer.reason.contains("MUSTER_PANE"), "{}", asked.answer.reason);
    let p1 = record(&snapshot(&mut control), "p1").clone();
    assert_eq!(p1.session_id.as_deref(), Some("s-5"), "the agent was left running: {p1:?}");
    drop(daemon);
}

/// A compaction still waiting for the agent goes with it, and the log says so: it is not typed
/// into the agent resuming in its place.
#[test]
fn a_compaction_waiting_for_a_restarted_agent_is_dropped_and_logged() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-6");
    daemon.set_agent_state("p1", proto::AgentState::Working);
    let compact = pane_request::Compact {
        pane: "p1".to_string(),
        focus: None,
        then: Some("carry on".to_string()),
    };
    expect(&mut control, pane(pane_request::Request::Compact(compact)), proto::Outcome::Done);

    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    let path = daemon.root().join("daemon.log");
    until(
        "the dropped compaction in the daemon's log",
        || {
            written(&path).lines().any(|line| {
                line.contains(r#""event":"daemon.compact.dropped""#)
                    && line.contains(r#""restarting":"true""#)
            })
        },
        (),
    );
    let p1 = record(&snapshot(&mut control), "p1").clone();
    assert_eq!(p1.pending_compaction(), proto::PendingCompaction::None, "{p1:?}");
}

fn session_and_resume(control: &mut Control) -> (Option<String>, bool) {
    let p1 = record(&snapshot(control), "p1").clone();
    (p1.session_id, p1.resume.is_some())
}

/// A harness that reports its session only at its first prompt, as Codex does, says nothing
/// after a restart until somebody types. The pane goes on showing the session it was restarted
/// in, so it can be restarted again before then.
#[test]
fn a_restarted_agent_that_has_not_reported_keeps_its_session_and_restarts_again() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-twice");
    let first = std::fs::read_to_string(home(&daemon, "fake-agent-pid")).expect("a pid");

    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);
    let kept = (Some("s-twice".to_string()), true);
    assert_eq!(session_and_resume(&mut control), kept, "as the restart answers");
    let second = written_again(&daemon, "fake-agent-pid", &first);
    daemon.until_agent("p1", proto::AgentState::Idle);
    assert_eq!(session_and_resume(&mut control), kept, "with the resumed agent back");

    let asked = expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    assert!(restarted(&asked).typed.ends_with("--model' 'opus' '--resume' 's-twice'"), "{asked:?}");
    assert_ne!(written_again(&daemon, "fake-agent-pid", &second), second);
    daemon.until_agent("p1", proto::AgentState::Idle);
    assert_eq!(session_and_resume(&mut control), kept, "after the second restart");
}

/// The session a restarted agent kept leaves with it, as any agent's does.
#[test]
fn a_restarted_agent_that_quits_before_reporting_takes_its_session_with_it() {
    let (daemon, mut control) = reported(&[], "--model opus", "s-quit");
    let before = std::fs::read_to_string(home(&daemon, "fake-agent-pid")).expect("a pid");
    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);
    written_again(&daemon, "fake-agent-pid", &before);
    daemon.until_agent("p1", proto::AgentState::Idle);

    daemon.type_into("p1", "quit");

    until_some("p1 to show no session once its agent quit", || {
        (session_and_resume(&mut control) == (None, false)).then_some(())
    });
}

/// A resume that starts no agent leaves a shell, and the pane stops showing a session nothing
/// runs once the wait for the agent is over.
#[test]
fn a_restart_whose_agent_never_comes_back_stops_showing_its_session() {
    let (daemon, mut control) =
        reported(&[("MUSTER_DAEMON_RESUME_KEPT_MS", "50")], "--model opus", "s-gone");
    std::fs::remove_file(daemon.agent_path()).expect("the test removes the agent's program");

    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);

    until_some("p1 to stop showing the session", || {
        (session_and_resume(&mut control) == (None, false)).then_some(())
    });
    let log = written(&daemon.root().join("daemon.log"));
    assert!(log.contains("its agent was restarted and did not come back"), "{log}");
}

/// Longer than any test runs, so a session kept for a restarted agent goes only by what the test
/// does.
const KEPT_FOR_THE_WHOLE_TEST: (&str, &str) = ("MUSTER_DAEMON_RESUME_KEPT_MS", "3600000");

/// The fake agent in `p1`, session reported, restarted with its program gone: the resume typed at
/// the shell starts nothing, and the pane goes on showing the session it was restarted in.
fn restarted_into_nothing(session: &str) -> (Daemon, Control) {
    let (daemon, mut control) = reported(&[KEPT_FOR_THE_WHOLE_TEST], "--model opus", session);
    let program = daemon.agent_path();
    let agent = std::fs::read_link(&program).expect("the fake agent is a link");
    std::fs::remove_file(&program).expect("the test removes the agent's program");
    expect(&mut control, restart("p1", &[]), proto::Outcome::Done);
    assert_eq!(session_and_resume(&mut control), (Some(session.to_string()), true));
    // However this shell says so: restoring the program any sooner lets the resume start it.
    until_some("the shell to say the resume started nothing", || {
        let screen = screen_text(&mut control, "p1");
        (screen.contains("not found") || screen.contains("No such file")).then_some(())
    });
    std::os::unix::fs::symlink(agent, &program).expect("the test puts the program back");
    (daemon, control)
}

/// The resume a restart typed can fail, and somebody then starts the harness afresh. That one
/// was not started in the old session, so the pane must not say it runs it: Restart Agent would
/// end the fresh session to resume the old one.
#[test]
fn a_fresh_agent_started_after_a_failed_resume_is_not_given_the_old_session() {
    let (daemon, mut control) = restarted_into_nothing("s-old");

    daemon.run_agent("p1");

    assert_eq!(session_and_resume(&mut control), (None, false));
}

/// A harness can report its session before detection has found it, and from a process whose
/// arguments say nothing of how to resume it. The pane then shows the session reported, and not
/// the resume kept for another.
#[test]
fn a_session_reported_after_a_restart_replaces_the_one_kept_for_it() {
    let (_daemon, mut control) = restarted_into_nothing("s-kept");

    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some("s-new".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);

    assert_eq!(session_and_resume(&mut control), (Some("s-new".to_string()), false));
}

/// A pane closed while it shows the session its agent was restarted in comes back in that
/// session, as one closed with its agent running does.
#[test]
fn a_pane_closed_during_a_restart_reopens_in_the_session_it_showed() {
    let (_daemon, mut control) = restarted_into_nothing("s-closed");

    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    crate::reopen::reopened(&mut control, "p1", "t2");

    let command = record(&snapshot(&mut control), "p1").command.clone().unwrap_or_default();
    assert!(command.ends_with("--model opus --resume s-closed"), "reopened as {command:?}");
}

/// An empty session id is a harness asking for its session to be forgotten, and that covers the
/// one a pane still shows for a restarted agent.
#[test]
fn an_empty_session_report_forgets_the_session_kept_for_a_restart() {
    let (_daemon, mut control) = restarted_into_nothing("s-forgotten");

    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some(String::new()),
        ..Default::default()
    };
    control.ask(pane(pane_request::Request::Report(report)));

    assert_eq!(session_and_resume(&mut control), (None, false));
}

/// A job the shell starts as the agent ends keeps the terminal, so the restart cannot type its
/// resume, and detection can go on naming the ended agent for a while. The restart knows the
/// agent ended: its answer leaves the pane showing no session, with the resume still saved for
/// a daemon that dies now.
#[test]
fn a_restart_that_could_not_type_its_resume_stops_showing_the_ended_agents_session() {
    let kept = [KEPT_FOR_THE_WHOLE_TEST];
    let (daemon, mut control) = reported(&kept, "--model opus; /bin/sleep 12", "s-untyped");

    let asked = control.ask(restart("p1", &[]));

    assert_ne!(asked.outcome(), proto::Outcome::Done, "the resume was typed: {asked:?}");
    assert_eq!(session_and_resume(&mut control), (None, false));
    let saved = written(&crate::persistence::state_file(&daemon));
    assert!(saved.contains("s-untyped"), "the state file lost the resume: {saved}");
}
