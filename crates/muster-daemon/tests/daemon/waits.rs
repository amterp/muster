//! A wait an agent declared that its next turn does not end: one until a process exits, which
//! ends when the process does and wakes the agent with a message from `@muster`. The agent is the
//! harness's fake under `claude`'s manifest, idle at its prompt, which writes down every line it
//! reads, so a wake typed into it is on disk.

use std::process::Command;
use std::time::Duration;

use crate::support::*;

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

    /// Declares a wait on `waiting`, until `until_exits` exits when it names a process.
    fn declares(&mut self, waiting: &str, until_exits: Option<u32>) -> Asked {
        let report = proto::pane_request::Report {
            pane: "p1".to_string(),
            waiting: Some(waiting.to_string()),
            waiting_until_exits: until_exits,
            ..Default::default()
        };
        self.control.ask(pane(proto::pane_request::Request::Report(report)))
    }

    /// Reports `state` as `claude`'s hooks do, with the empty wait its prompt hook sends.
    fn hook_reports(&mut self, state: proto::AgentState) {
        let mut report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            waiting: (state == proto::AgentState::Working).then(String::new),
            ..Default::default()
        };
        report.set_state(state);
        expect(
            &mut self.control,
            pane(proto::pane_request::Request::Report(report)),
            proto::Outcome::Done,
        );
    }

    /// What the agent waits on, and the process whose exit ends it.
    fn waiting_on(&mut self) -> Option<(String, Option<u32>)> {
        let records = snapshot(&mut self.control).panes;
        let facts = records.into_iter().find(|record| record.pane == "p1")?.facts?;
        Some((facts.waiting?, facts.waiting_until_exits))
    }

    /// Every wake the doorbell typed into the agent.
    fn rings_heard(&self) -> Vec<String> {
        std::fs::read_to_string(self.daemon.root().join("home/fake-agent-heard"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("[muster] "))
            .map(str::to_string)
            .collect()
    }
}

/// A process that runs until it is killed, as a background job does until it is done.
fn job() -> std::process::Child {
    Command::new("sleep").arg("600").spawn().expect("sleep runs")
}

#[test]
fn a_wait_until_a_process_exits_ends_with_it_and_wakes_the_agent() {
    let mut p1 = Pane::with_an_agent();
    let mut job = job();
    let asked = p1.declares("the full gate", Some(job.id()));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), Some(job.id()))));

    std::thread::sleep(Duration::from_secs(3));
    assert!(p1.waiting_on().is_some(), "ended with its process still running");
    assert_eq!(p1.rings_heard(), Vec::<String>::new());

    job.kill().expect("the job is killed");
    job.wait().expect("and reaped");
    until_some("the wait to end with its process", || p1.waiting_on().is_none().then_some(()));
    let rings = until_some("the agent to be woken", || {
        Some(p1.rings_heard()).filter(|rings| !rings.is_empty())
    });
    assert!(rings[0].contains("from @muster"), "{rings:?}");
}

/// Neither a turn ending nor a prompt's hook ends a wait on a process; the agent saying no wait
/// itself does.
#[test]
fn a_wait_on_a_process_outlasts_turns_and_prompts() {
    use proto::AgentState::{Idle, Working};
    let mut p1 = Pane::with_an_agent();
    let mut job = job();
    let asked = p1.declares("the full gate", Some(job.id()));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    p1.hook_reports(Working);
    p1.hook_reports(Idle);
    p1.hook_reports(Working);
    p1.hook_reports(Idle);
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), Some(job.id()))));

    let asked = p1.declares("", None);
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    assert_eq!(p1.waiting_on(), None, "the agent said it waits on nothing");
    job.kill().expect("the job is killed");
    job.wait().expect("and reaped");
}

#[test]
fn a_wait_on_a_process_already_gone_is_refused() {
    let mut p1 = Pane::with_an_agent();
    let mut done = Command::new("true").spawn().expect("true runs");
    done.wait().expect("and is reaped");
    let asked = p1.declares("the full gate", Some(done.id()));
    assert_eq!(asked.outcome(), proto::Outcome::Refused);
    assert!(asked.answer.reason.contains("already exited"), "{}", asked.answer.reason);
    assert_eq!(p1.waiting_on(), None);
}
