//! Restarting a pane's agent where it runs (`pane.restart`): its input emptied, its program
//! ended, and its session's resume typed at the shell it leaves behind.

use std::path::PathBuf;

use crate::persistence::{record, until_saved};
use crate::support::*;
use proto::pane_request;

/// The fake agent in `p1` of a fresh detecting daemon, started with `arguments`, its session
/// reported as `session`.
fn reported(environment: &[(&str, &str)], arguments: &str, session: &str) -> (Daemon, Control) {
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
