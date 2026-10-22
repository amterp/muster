//! A wait an agent declared that its next turn does not end: one until a process exits, which
//! ends when the process does and wakes the agent with a message from `@muster`, and one that runs
//! out, from a harness nothing wakes or carried through a turn Muster typed. The agent is the
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
        Pane::with_an_agent_given(&[])
    }

    /// [`Pane::with_an_agent`], with a declared wait that nothing ends lasting [`LIMIT_MS`].
    fn with_a_short_limit() -> Pane {
        Pane::with_an_agent_given(&[("MUSTER_DAEMON_DECLARED_WAIT_MS", LIMIT_MS)])
    }

    fn with_an_agent_given(environment: &[(&str, &str)]) -> Pane {
        let daemon = Daemon::start_detecting_with(environment);
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

    /// Has the agent's manifest say nothing wakes it when its jobs end, as Codex's does not.
    fn unwoken_by_its_jobs(&mut self) {
        let path = self.daemon.root().join("home/.muster/agent-detection/claude.toml");
        let manifest = std::fs::read_to_string(&path).expect("the harness wrote it");
        let unwoken: String = manifest
            .lines()
            .filter(|line| !line.starts_with("wakes_when_jobs_end"))
            .flat_map(|line| [line, "\n"])
            .collect();
        assert_ne!(unwoken, manifest, "the harness's manifest says its jobs wake it");
        std::fs::write(&path, unwoken).expect("the override is rewritten");
        let send = session(proto::session_request::Request::SendManifests(
            proto::SendManifests::default(),
        ));
        let asked = self.control.ask(send);
        assert!(
            matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo),
            "sending the manifests was refused: {}",
            asked.answer.reason
        );
    }

    /// Declares a wait in a turn of the agent's, as an agent does before ending it to wait.
    fn turn_declaring(&mut self, waiting: &str) {
        self.hook_reports(proto::AgentState::Working);
        let asked = self.declares(waiting, None);
        assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
        self.hook_reports(proto::AgentState::Idle);
        assert_eq!(self.waiting_on(), Some((waiting.to_string(), None)));
    }

    fn finished_unseen(&mut self) -> bool {
        let records = snapshot(&mut self.control).panes;
        records.into_iter().find(|record| record.pane == "p1").is_some_and(|r| r.finished_unseen)
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

/// How long a wait nothing ends lasts here, in milliseconds.
const LIMIT_MS: &str = "3000";

/// Longer than [`LIMIT_MS`], with room for a look of the doorbell's thread.
const PAST_THE_LIMIT: Duration = Duration::from_secs(5);

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

/// A wait whose process exited stands while the agent cannot be told, and ends once it is: a
/// wait cleared with its wake refused would leave the agent idle with nothing to start its turn.
#[test]
fn a_wait_whose_wake_cannot_be_posted_stands_until_it_is() {
    use std::os::unix::fs::PermissionsExt;
    let mut p1 = Pane::with_an_agent();
    let mut job = job();
    let asked = p1.declares("the full gate", Some(job.id()));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    // The message store, made before its first write and left unwritable, refuses the wake.
    let store = p1.daemon.socket_path().with_extension("msg");
    std::fs::create_dir_all(&store).expect("the store's directory is made");
    let writable = |mode| {
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&store, permissions).expect("the store's mode is set");
    };
    writable(0o555);

    job.kill().expect("the job is killed");
    job.wait().expect("and reaped");
    std::thread::sleep(Duration::from_secs(4));
    let standing = p1.waiting_on();
    writable(0o755);
    assert_eq!(standing, Some(("the full gate".to_string(), Some(job.id()))), "ended untold");
    assert_eq!(p1.rings_heard(), Vec::<String>::new());

    until_some("the wait to end once its wake is posted", || {
        p1.waiting_on().is_none().then_some(())
    });
    let rings = until_some("the agent to be woken", || {
        Some(p1.rings_heard()).filter(|rings| !rings.is_empty())
    });
    assert!(rings[0].contains("from @muster"), "{rings:?}");
}

/// A wait whose agent can never be told is given up after the limit a wait nothing ends has:
/// the pane reads idle again rather than waiting for good, which somebody notices.
#[test]
fn a_wait_whose_wake_is_refused_for_good_ends_after_the_limit() {
    use std::os::unix::fs::PermissionsExt;
    let mut p1 = Pane::with_a_short_limit();
    let mut job = job();
    let asked = p1.declares("the full gate", Some(job.id()));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    let store = p1.daemon.socket_path().with_extension("msg");
    std::fs::create_dir_all(&store).expect("the store's directory is made");
    let unwritable = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(&store, unwritable).expect("the store's mode is set");

    job.kill().expect("the job is killed");
    job.wait().expect("and reaped");
    std::thread::sleep(Duration::from_secs(1));
    assert!(p1.waiting_on().is_some(), "ended untold at once");
    until_some("the wait to be given up", || p1.waiting_on().is_none().then_some(()));
    let writable = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&store, writable).expect("the store's mode is set back");
    assert_eq!(p1.rings_heard(), Vec::<String>::new());
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

/// A wait from a harness that nothing wakes when its jobs end runs out, and the turn it held back
/// is finished, so the pane reads idle and a helper's maker hears of it.
#[test]
fn a_wait_from_a_harness_its_jobs_do_not_wake_runs_out() {
    let mut p1 = Pane::with_a_short_limit();
    p1.unwoken_by_its_jobs();
    p1.turn_declaring("the full gate");
    assert!(!p1.finished_unseen(), "waiting is not done");
    until_some("the wait to run out", || p1.waiting_on().is_none().then_some(()));
    assert!(p1.finished_unseen(), "the turn it held back is finished");
}

/// Claude Code's own jobs wake it, so a wait it declared lasts until a turn ends without it; one
/// carried through a compaction Muster typed runs out, since the job may have woken it inside
/// that turn, after which no turn comes to end the wait.
#[test]
fn a_wait_its_jobs_end_has_no_limit_until_it_is_carried() {
    use proto::AgentState::{Idle, Working};
    let mut p1 = Pane::with_a_short_limit();
    p1.turn_declaring("the full gate");
    std::thread::sleep(PAST_THE_LIMIT);
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), None)), "it ran out");

    let compact = proto::pane_request::Compact {
        pane: "p1".to_string(),
        focus: Some("keep the work notes".to_string()),
        then: None,
    };
    let asked = p1.control.ask(pane(proto::pane_request::Request::Compact(compact)));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    until_some("the compaction to be typed", || {
        let heard = std::fs::read_to_string(p1.daemon.root().join("home/fake-agent-heard"));
        heard.ok().filter(|heard| heard.contains("/compact keep the work notes"))
    });
    p1.hook_reports(Working);
    p1.hook_reports(Idle);
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), None)), "carried");
    until_some("the carried wait to run out", || p1.waiting_on().is_none().then_some(()));
    assert!(p1.finished_unseen(), "the turn it held back is finished");
}
