//! What an agent reports about itself, from inside its pane: its context, sub-agents, model and
//! cost, kept on the pane's record (`PaneRequest.Report`, `muster-daemon report`).

use std::process::Command;

use crate::support::*;
use proto::event::Event as Payload;

fn report(pane: &str) -> proto::pane_request::Report {
    proto::pane_request::Report { pane: pane.to_string(), ..Default::default() }
}

fn report_request(report: proto::pane_request::Report) -> proto::request::Service {
    pane(proto::pane_request::Request::Report(report))
}

fn facts_of(control: &mut Control, name: &str) -> Option<proto::AgentFacts> {
    snapshot(control).panes.into_iter().find(|record| record.pane == name).and_then(|p| p.facts)
}

#[test]
fn a_report_lands_on_the_panes_record_and_is_announced() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    expect(&mut control, subscribe_request(), proto::Outcome::Done);

    let first = proto::pane_request::Report {
        context_used: Some(42.0),
        model: Some("Opus".to_string()),
        cost_usd: Some(1.25),
        subagent: proto::SubagentChange::Started.into(),
        facts: [("branch".to_string(), "main".to_string())].into(),
        ..report("p1")
    };
    let asked = expect(&mut control, report_request(first.clone()), proto::Outcome::Done);
    let announced = asked.events.iter().find_map(|event| match &event.event {
        Some(Payload::PaneChanged(changed)) => changed.pane.as_ref().and_then(|p| p.facts.clone()),
        _ => None,
    });
    let facts = announced.expect("the change was announced before the answer");
    assert_eq!(facts.context_used, Some(42.0));
    assert_eq!(facts.model.as_deref(), Some("Opus"));
    assert_eq!(facts.cost_usd, Some(1.25));
    assert_eq!(facts.subagents, 1);
    assert_eq!(facts.other.get("branch").map(String::as_str), Some("main"));
    assert_eq!(facts_of(&mut control, "p1"), Some(facts));

    let same = proto::pane_request::Report { subagent: 0, ..first };
    expect(&mut control, report_request(same), proto::Outcome::AlreadySo);
    let stopped = proto::pane_request::Report {
        subagent: proto::SubagentChange::Stopped.into(),
        ..report("p1")
    };
    expect(&mut control, report_request(stopped), proto::Outcome::Done);
    assert_eq!(facts_of(&mut control, "p1").map(|facts| facts.subagents), Some(0));
}

#[test]
fn a_report_for_no_pane_or_out_of_bounds_is_refused() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    expect(&mut control, report_request(report("nope")), proto::Outcome::NotThere);
    let over = proto::pane_request::Report { context_used: Some(140.0), ..report("p1") };
    let refused = expect(&mut control, report_request(over), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("context_used"), "{}", refused.answer.reason);
    assert_eq!(facts_of(&mut control, "p1"), None);
}

#[test]
fn a_program_in_a_pane_reports_through_the_environment_its_daemon_gave_it() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let command = "\"$MUSTER_DAEMON\" report --model Sonnet --context-used 7 --subagent-started \
                   && echo reported";
    make(
        &mut control,
        proto::pane_request::Create {
            command: Some(command.to_string()),
            ..create("p1", in_new_tab("t1"))
        },
    );
    until_text(&mut control, "p1", "reported");
    let facts = facts_of(&mut control, "p1").expect("the report arrived");
    assert_eq!(
        (facts.model.as_deref(), facts.context_used, facts.subagents),
        (Some("Sonnet"), Some(7.0), 1)
    );
}

#[test]
fn the_verb_says_why_when_it_cannot_report() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let run = |environment: &[(&str, &str)]| {
        let output = Command::new(env!("CARGO_BIN_EXE_muster-daemon"))
            .args(["report", "--model", "Opus"])
            .env_clear()
            .envs(environment.iter().copied())
            .output()
            .expect("the verb runs");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let socket = daemon.socket_path().to_str().unwrap();

    let (status, said) = run(&[("MUSTER_PANE", "p1")]);
    assert_eq!(status, Some(1));
    assert!(said.contains("MUSTER_DAEMON_SOCKET"), "{said}");

    let (status, said) = run(&[("MUSTER_PANE", "gone"), ("MUSTER_DAEMON_SOCKET", socket)]);
    assert_eq!(status, Some(1));
    assert!(said.contains("no pane gone"), "{said}");

    let (status, said) = run(&[("MUSTER_PANE", "p1"), ("MUSTER_DAEMON_SOCKET", socket)]);
    assert_eq!((status, said.as_str()), (Some(0), ""), "a report that lands is silent");
    assert_eq!(facts_of(&mut control, "p1").and_then(|facts| facts.model).as_deref(), Some("Opus"));
}
