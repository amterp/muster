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

/// Waits for the daemon's first manifest load. Until it lands no sender can be attributed, and a
/// report is taken from anywhere, so a case about a pane with no agent starts after it; a case
/// that runs the agent first has already waited, since detecting the agent needs them too.
fn until_detecting(daemon: &Daemon) {
    until(
        "the daemon's first manifest load",
        || written(&daemon.root().join("daemon.log")).contains("daemon.detect.loaded"),
        (),
    );
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
    until_detecting(&daemon);
    // In the background, so the pane's foreground stays its shell and no agent is found there.
    let agent = daemon.agent_path();
    daemon.type_into("p1", &format!("{} nested-report --context-used 55 &", agent.display()));
    assert_eq!(until_reported(&daemon, 1), ["nested 0"]);
    assert_eq!(context_of(&mut control, "p1"), Some(55.0));
}

fn served_said(daemon: &Daemon) -> String {
    std::fs::read_to_string(daemon.root().join("home/fake-agent-served-said")).unwrap_or_default()
}

/// A report through the agent's server is refused even with the pane's own agent above it, as
/// Codex's shared app-server is its first session's child: the server runs every session's
/// hooks with that session's environment, so the pane it names says nothing.
#[test]
fn a_report_through_the_agents_server_is_refused_and_says_what_to_do() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    daemon.type_into("p1", "hook --context-used 42");
    assert_eq!(until_reported(&daemon, 1), ["hook 0"]);

    daemon.type_into("p1", "served --context-used 77");
    assert_eq!(until_reported(&daemon, 2)[1], "served 1", "a report through a server was taken");
    assert_eq!(context_of(&mut control, "p1"), Some(42.0), "the server's report's facts landed");
    let said = served_said(&daemon);
    assert!(said.contains("`claude serve`"), "the refusal does not name the server: {said}");
    assert!(said.contains("run the fake agent without its server"), "no remedy: {said}");
}

/// The same report is refused in a pane with no agent, which takes any other report: what it
/// names is no more that pane than any other.
#[test]
fn a_pane_with_no_agent_refuses_a_report_through_a_server() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    until_detecting(&daemon);
    let agent = daemon.agent_path();
    daemon.type_into("p1", &format!("{} serve --context-used 55 &", agent.display()));
    assert_eq!(until_reported(&daemon, 1), ["served 1"]);
    assert_eq!(context_of(&mut control, "p1"), None);
}

/// A report that comes before the daemon's first manifest load cannot be attributed, so it is
/// taken, even through a server, and the daemon says why once. The load here reads an override
/// that never ends, as one on a mount that hangs does: a FIFO nobody opens.
#[test]
fn a_report_before_the_manifests_load_is_taken_and_says_why() {
    let home = std::env::temp_dir().join(format!("muster-attribution-{}-hung", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("agent-detection")).unwrap();
    let fifo = home.join("agent-detection/claude.toml");
    let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo reads a NUL-terminated path the test owns.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let daemon = Daemon::start_detecting_with(&[("MUSTER_HOME", home.to_str().unwrap())]);
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");

    let agent = daemon.agent_path();
    daemon.type_into("p1", &format!("{} serve --context-used 55 &", agent.display()));
    let said = until_reported(&daemon, 1);
    let log = written(&daemon.root().join("daemon.log"));
    // Lets the load finish, whatever the verdict.
    drop(std::fs::OpenOptions::new().write(true).open(&fifo));
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(said, ["served 0"], "a report nothing could attribute was refused");
    let logged = |event: &str| log.contains(&format!("\"event\":\"{event}\""));
    assert!(logged("daemon.report.unattributed"), "the daemon did not say why: {log}");
    assert!(!logged("daemon.detect.loaded"), "the manifests loaded first: {log}");
}
