//! A report taken only from the pane's own agent (MIP-3, section 8). The fake agent, under
//! `claude`'s manifest, runs a report as a hook would, a child in its own process group, or
//! through a copy of itself started in a group of its own, as an agent's Bash tool starts a
//! nested `claude -p`. What each report said is told by the pane's facts, and whether the daemon
//! took it by the report's exit status, which the fake writes down.

use std::path::PathBuf;

use crate::support::*;

fn reported_file(daemon: &Daemon) -> PathBuf {
    daemon.root().join("home/fake-agent-reported")
}

fn reported(daemon: &Daemon) -> Vec<String> {
    std::fs::read_to_string(reported_file(daemon))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn until_reported(daemon: &Daemon, count: usize) -> Vec<String> {
    until_some(&format!("{count} reports to have answered"), || {
        let said = reported(daemon);
        (said.len() >= count).then_some(said)
    })
}

fn context_of(control: &mut Control, pane: &str) -> Option<f32> {
    snapshot(control)
        .panes
        .into_iter()
        .find(|record| record.pane == pane)
        .and_then(|record| record.facts)
        .and_then(|facts| facts.context_used)
}

#[test]
fn a_nested_agents_report_is_refused_and_the_panes_own_hook_is_taken() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Working);

    daemon.type_into("p1", "hook --agent claude --context-used 42");
    assert_eq!(until_reported(&daemon, 1), ["hook 0"], "the pane's own hook was refused");
    assert_eq!(context_of(&mut control, "p1"), Some(42.0));

    daemon.type_into("p1", "nested --agent claude --state idle --context-used 77");
    assert_eq!(until_reported(&daemon, 2)[1], "nested 1", "a nested agent's report was taken");
    assert_eq!(context_of(&mut control, "p1"), Some(42.0), "the nested report's facts landed");
    daemon.until_agent("p1", proto::AgentState::Working);

    daemon.type_into("p1", "hook --context-used 43");
    assert_eq!(until_reported(&daemon, 3)[2], "hook 0", "refusing one sender refused the pane");
    assert_eq!(context_of(&mut control, "p1"), Some(43.0));
}

/// A pane whose agent nobody has identified - a plain shell - takes a report from anywhere, as
/// every pane did before: there is nothing to tell its own agent's reports apart by.
#[test]
fn a_pane_with_no_agent_takes_a_report_from_an_agent_process() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    // In the background, so the pane's foreground stays its shell and no agent is found there.
    let agent = daemon.agent_path();
    daemon.type_into("p1", &format!("{} nested-report --context-used 55 &", agent.display()));
    assert_eq!(until_reported(&daemon, 1), ["nested 0"]);
    assert_eq!(context_of(&mut control, "p1"), Some(55.0));
}
