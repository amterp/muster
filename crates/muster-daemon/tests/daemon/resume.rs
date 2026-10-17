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
    let p1 = record(&snapshot(&mut control), "p1").clone();
    assert_eq!(p1.session_id.as_deref(), Some("s-1"), "the restored record names its session");
    assert!(p1.resume.is_some(), "the restored record says how it resumes: {p1:?}");
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

/// A word that could be a prompt or a flag's value carries none of the rest, rather than risk
/// sending a prompt twice - but the model and effort go along all the same, since without them
/// the session comes back on the settings' defaults.
#[test]
fn arguments_that_cannot_be_told_from_a_prompt_carry_only_the_model_and_effort() {
    let (mut daemon, mut control) = reported("--add-dir a b --model opus --effort high", "s-2");

    let mut control = restarted(&mut daemon, &mut control);

    let p1 = until_some("p1 to come back running its agent", || {
        Some(record(&snapshot(&mut control), "p1").clone()).filter(|p1| p1.command.is_some())
    });
    let command = p1.command.unwrap_or_default();
    assert!(
        command.ends_with("claude --model opus --effort high --resume s-2"),
        "resumed as {command:?}"
    );
    let uncarried = p1.resume.and_then(|resume| resume.uncarried);
    assert!(uncarried.is_some_and(|why| why.contains("`a`")), "the record says why the rest went");
}

/// A Claude Code started through a wrapper comes back through the wrapper, which sets up again
/// what it set up the first time: here a settings file it wrote for the agent and that is gone
/// by the restart, as a wrapper deleting its temporary `--settings` leaves it.
///
/// macOS does not show the environment of its own binaries, and the fake agent runs under
/// `/bin/sh`, so `MUSTER_RESUME` cannot be read off it there; a real agent is not one.
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS hides the environment of /bin/sh, which the fake agent runs under"
)]
fn an_agent_started_through_a_wrapper_comes_back_through_it() {
    let mut daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let wrapper = daemon.root().join("wrapper");
    let settings = daemon.root().join("wrapper-settings.json");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '{{}}' > '{settings}'\nexport MUSTER_RESUME='{wrapper} --resume \
             {{session}}'\nexec '{agent}' --settings '{settings}' \"$@\"\n",
            settings = settings.display(),
            wrapper = wrapper.display(),
            agent = daemon.agent_path().display(),
        ),
    )
    .expect("the test writes its wrapper");
    std::fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("the wrapper is executable");
    until_text(&mut control, "p1", "$");
    daemon.type_into("p1", &wrapper.display().to_string());
    until_text(&mut control, "p1", "PROBE-STATE:IDLE");
    daemon.until_agent("p1", proto::AgentState::Idle);
    let report = pane_request::Report {
        pane: "p1".to_string(),
        agent: "claude".to_string(),
        session_id: Some("s-7".to_string()),
        ..Default::default()
    };
    expect(&mut control, pane(pane_request::Request::Report(report)), proto::Outcome::Done);
    until_saved(&daemon, "s-7");
    std::fs::remove_file(&settings).expect("the test removes the wrapper's settings");

    let mut control = restarted(&mut daemon, &mut control);

    let command = until_some("p1 to come back running its agent", || {
        record(&snapshot(&mut control), "p1").command.clone()
    });
    assert!(
        command.ends_with(&format!("{} --resume s-7", wrapper.display())),
        "resumed as {command:?}"
    );
    daemon.until_agent("p1", proto::AgentState::Idle);
    assert!(settings.exists(), "the wrapper ran again and wrote its settings again");
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

/// The pane's record says which session its agent runs and how the daemon would resume it, for
/// a client to show, and says neither once the agent has left.
#[test]
fn a_panes_record_carries_its_session_and_resume_while_its_agent_runs() {
    let (daemon, mut control) = reported("--model opus", "s-5");

    let p1 = record(&snapshot(&mut control), "p1").clone();
    assert_eq!(p1.session_id.as_deref(), Some("s-5"), "{p1:?}");
    let resume = p1.resume.unwrap_or_else(|| panic!("no resume on p1's record"));
    assert!(
        resume.command.join(" ").ends_with("claude --model opus --resume s-5"),
        "{:?}",
        resume.command
    );
    assert_eq!(resume.uncarried, None);

    daemon.type_into("p1", "quit");
    until_some("p1's agent to leave and take its session with it", || {
        let p1 = record(&snapshot(&mut control), "p1").clone();
        (p1.agent.is_none() && p1.session_id.is_none() && p1.resume.is_none()).then_some(())
    });
}
