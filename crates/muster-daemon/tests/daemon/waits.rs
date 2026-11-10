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

    fn declares_from(&mut self, sender: &str) -> Asked {
        self.control.ask(pane(proto::pane_request::Request::Report(proto::pane_request::Report {
            pane: "p1".to_string(),
            waiting: Some("the review".to_string()),
            waiting_until_message_from: Some(sender.to_string()),
            ..Default::default()
        })))
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

    fn until_wait_over(&mut self) {
        let log = self.daemon.root().join("daemon.log");
        until(
            "the declared wait to end",
            || self.waiting_on().is_none(),
            || {
                let log = std::fs::read_to_string(&log).unwrap_or_default();
                log.lines()
                    .filter(|line| !line.contains("daemon.request.answered"))
                    .rev()
                    .take(30)
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );
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

    // With `work` in its focus the fake agent would read working on screen for good while its
    // hook says idle; whether the pane then read idle as the wait ran out would turn on whether
    // detection had read that screen before the hook's report came.
    let compact = proto::pane_request::Compact {
        pane: "p1".to_string(),
        focus: Some("keep the notes".to_string()),
        then: None,
    };
    let asked = p1.control.ask(pane(proto::pane_request::Request::Compact(compact)));
    assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    until_some("the compaction to be typed", || {
        let heard = std::fs::read_to_string(p1.daemon.root().join("home/fake-agent-heard"));
        heard.ok().filter(|heard| heard.contains("/compact keep the notes"))
    });
    p1.hook_reports(Working);
    p1.hook_reports(Idle);
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), None)), "carried");
    until_some("the carried wait to run out", || p1.waiting_on().is_none().then_some(()));
    assert!(p1.finished_unseen(), "the turn it held back is finished");
}

/// A wait carried through a turn Muster typed is still limited in the daemon a pane is handed
/// to: taken over as the agent's own, it would last until a turn that may never come.
#[test]
fn a_carried_wait_is_still_limited_after_a_handoff() {
    use proto::AgentState::{Idle, Working};
    let mut p1 = Pane::with_a_short_limit();
    p1.turn_declaring("the full gate");
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

    let answer = p1.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    p1.control = p1.daemon.connect();
    assert_eq!(p1.waiting_on(), Some(("the full gate".to_string(), None)), "handed over");
    until_some("the carried wait to run out in the new daemon", || {
        p1.waiting_on().is_none().then_some(())
    });
}

#[test]
fn a_reviewers_message_ends_the_wait_before_the_worker_reports_completion() {
    use proto::AgentState::{Idle, Working};
    use proto::msg_request::{self, Request};
    let mut worker = Pane::with_an_agent();
    make(&mut worker.control, create("p2", in_new_tab("t2")));
    worker.daemon.run_agent("p2");
    worker.hook_reports(Working);
    let declared = worker.declares_from("p2");
    assert_eq!(declared.outcome(), proto::Outcome::Done, "{}", declared.answer.reason);
    worker.hook_reports(Idle);
    let message = |from: &str, request| {
        proto::request::Service::Msg(proto::MsgRequest {
            caller: Some(msg_request::Caller {
                pane: Some(from.to_string()),
                ..Default::default()
            }),
            request: Some(request),
        })
    };
    expect(
        &mut worker.control,
        message(
            "p2",
            Request::Post(msg_request::Post {
                to: vec!["p1".to_string()],
                body: "review complete".to_string(),
                ..Default::default()
            }),
        ),
        proto::Outcome::Done,
    );
    until_some("the review wake to reach the worker", || {
        (!worker.rings_heard().is_empty()).then_some(())
    });
    worker.hook_reports(Working);
    expect(
        &mut worker.control,
        message("p1", Request::Read(msg_request::Read::default())),
        proto::Outcome::Done,
    );
    expect(
        &mut worker.control,
        message(
            "p1",
            Request::Post(msg_request::Post {
                to: vec!["p2".to_string()],
                body: "work complete".to_string(),
                ..Default::default()
            }),
        ),
        proto::Outcome::Done,
    );
    worker.hook_reports(Idle);
    assert_eq!(worker.waiting_on(), None, "the review arrived and the worker finished");
}

fn message(from: &str, request: proto::msg_request::Request) -> proto::request::Service {
    proto::request::Service::Msg(proto::MsgRequest {
        caller: Some(proto::msg_request::Caller {
            pane: Some(from.to_string()),
            ..Default::default()
        }),
        request: Some(request),
    })
}

fn review_from(from: &str) -> proto::request::Service {
    message(
        from,
        proto::msg_request::Request::Post(proto::msg_request::Post {
            to: vec!["p1".to_string()],
            body: "review complete".to_string(),
            ..Default::default()
        }),
    )
}

#[test]
fn an_unrelated_message_keeps_a_sender_wait_and_a_renamed_reviewers_message_ends_it() {
    use proto::AgentState::{Idle, Working};
    use proto::msg_request::{self, Request};
    let mut worker = Pane::with_an_agent();
    for (pane, tab) in [("p2", "t2"), ("p3", "t3")] {
        make(&mut worker.control, create(pane, in_new_tab(tab)));
        worker.daemon.run_agent(pane);
    }
    let records = snapshot(&mut worker.control).panes;
    let reviewer = records.iter().find(|r| r.pane == "p2").unwrap().callsign.as_deref().unwrap();
    worker.hook_reports(Working);
    assert_eq!(worker.declares_from(reviewer).outcome(), proto::Outcome::Done);
    worker.hook_reports(Idle);
    expect(&mut worker.control, review_from("p3"), proto::Outcome::Done);
    until_some("the unrelated wake", || (!worker.rings_heard().is_empty()).then_some(()));
    worker.hook_reports(Working);
    expect(
        &mut worker.control,
        message("p1", Request::Read(msg_request::Read::default())),
        proto::Outcome::Done,
    );
    worker.hook_reports(Idle);
    assert!(worker.waiting_on().is_some(), "the unrelated sender ended the wait");
    expect(
        &mut worker.control,
        pane(proto::pane_request::Request::Rename(proto::pane_request::Rename {
            pane: "p2".to_string(),
            label: Some("finished reviewer".to_string()),
        })),
        proto::Outcome::Done,
    );
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    worker.until_wait_over();
}

#[test]
fn a_review_wait_survives_handoff_and_waits_for_delivery_past_a_dialog() {
    use proto::AgentState::{Blocked, Idle};
    let mut worker = Pane::with_an_agent();
    make(&mut worker.control, create("p2", in_new_tab("t2")));
    worker.daemon.run_agent("p2");
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    let replaced = worker.daemon.replace(None);
    assert_eq!(replaced.outcome(), proto::Outcome::Done, "{}", replaced.reason);
    worker.control = worker.daemon.connect();
    worker.daemon.set_agent_state("p1", Blocked);
    worker.hook_reports(Blocked);
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    assert!(worker.waiting_on().is_some(), "posting is not delivery");
    worker.daemon.set_agent_state("p1", Idle);
    worker.hook_reports(Idle);
    worker.until_wait_over();
}

#[test]
fn hooks_reading_the_requested_senders_message_end_the_wait() {
    use proto::msg_request::{self, Request};
    let mut worker = Pane::with_an_agent();
    make(&mut worker.control, create("p2", in_new_tab("t2")));
    worker.daemon.run_agent("p2");
    expect(
        &mut worker.control,
        message(
            "p1",
            Request::Join(msg_request::Join {
                name: None,
                group: Some("reviews".to_string()),
                pull: true,
            }),
        ),
        proto::Outcome::Done,
    );
    worker.daemon.set_agent_state("p1", proto::AgentState::Working);
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    assert!(worker.waiting_on().is_some(), "the hooks have not fetched it yet");
    expect(
        &mut worker.control,
        message("p1", Request::Read(msg_request::Read::default())),
        proto::Outcome::Done,
    );
    assert!(worker.waiting_on().is_none());
}

#[test]
fn a_requested_senders_message_delivered_by_session_command_ends_the_wait() {
    let mut worker = Pane::with_an_agent();
    make(&mut worker.control, create("p2", in_new_tab("t2")));
    expect(
        &mut worker.control,
        pane(proto::pane_request::Request::Report(proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            session_id: Some("review-session".to_string()),
            ..Default::default()
        })),
        proto::Outcome::Done,
    );
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    expect(
        &mut worker.control,
        proto::request::Service::Msg(proto::MsgRequest {
            caller: Some(proto::msg_request::Caller {
                as_name: Some("p2".to_string()),
                ..Default::default()
            }),
            request: Some(proto::msg_request::Request::Post(proto::msg_request::Post {
                to: vec!["p1".to_string()],
                body: "review complete".to_string(),
                ..Default::default()
            })),
        }),
        proto::Outcome::Done,
    );
    worker.until_wait_over();
    assert!(worker.rings_heard().is_empty(), "the wake was typed instead of queued");
    assert!(
        std::fs::read_to_string(worker.daemon.root().join("home/fake-agent-woken"))
            .unwrap()
            .contains("review-session|")
    );
}

#[test]
fn sender_waits_refuse_unknown_ambiguous_and_self_targets_without_changing_facts() {
    let mut worker = Pane::with_an_agent();
    for sender in ["missing", "", "p1"] {
        let answer = worker.declares_from(sender);
        assert_eq!(answer.outcome(), proto::Outcome::Refused, "{sender}: {}", answer.answer.reason);
        assert_eq!(worker.waiting_on(), None);
    }
    for (pane_id, tab) in [("p2", "t2"), ("p3", "t3")] {
        make(&mut worker.control, create(pane_id, in_new_tab(tab)));
        expect(
            &mut worker.control,
            pane(proto::pane_request::Request::Rename(proto::pane_request::Rename {
                pane: pane_id.to_string(),
                label: Some("reviewer".to_string()),
            })),
            proto::Outcome::Done,
        );
    }
    assert_eq!(worker.declares_from("reviewer").outcome(), proto::Outcome::Refused);
}

/// Pane `p1` and the panes `others`, each with an agent, and `p1` working: a turn in which what
/// `p1` reads is read in the turn it declares in.
fn worker_beside(others: &[(&str, &str)]) -> Pane {
    let mut worker = Pane::with_an_agent();
    for &(pane, tab) in others {
        make(&mut worker.control, create(pane, in_new_tab(tab)));
        worker.daemon.run_agent(pane);
    }
    worker.hook_reports(proto::AgentState::Working);
    worker
}

fn reads(worker: &mut Pane) {
    expect(
        &mut worker.control,
        message("p1", proto::msg_request::Request::Read(proto::msg_request::Read::default())),
        proto::Outcome::Done,
    );
}

#[test]
fn an_answer_read_before_the_wait_is_declared_leaves_nothing_to_wait_for() {
    let mut worker = worker_beside(&[("p2", "t2")]);
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    reads(&mut worker);
    let declared = worker.declares_from("p2");
    assert_eq!(declared.outcome(), proto::Outcome::AlreadySo, "{}", declared.answer.reason);
    worker.hook_reports(proto::AgentState::Idle);
    assert_eq!(worker.waiting_on(), None, "p2 had answered in this turn");
}

#[test]
fn an_answer_read_in_an_earlier_turn_does_not_end_a_later_wait() {
    let mut worker = worker_beside(&[("p2", "t2")]);
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    reads(&mut worker);
    worker.hook_reports(proto::AgentState::Idle);
    worker.hook_reports(proto::AgentState::Working);
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    worker.hook_reports(proto::AgentState::Idle);
    assert!(worker.waiting_on().is_some(), "p2's earlier answer ended a wait for the next one");
}

#[test]
fn an_answer_read_before_posting_a_follow_up_does_not_end_a_wait_for_its_answer() {
    let mut worker = worker_beside(&[("p2", "t2")]);
    expect(&mut worker.control, review_from("p2"), proto::Outcome::Done);
    reads(&mut worker);
    expect(
        &mut worker.control,
        message(
            "p1",
            proto::msg_request::Request::Post(proto::msg_request::Post {
                to: vec!["p2".to_string()],
                body: "fixed, please check again".to_string(),
                ..Default::default()
            }),
        ),
        proto::Outcome::Done,
    );
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    worker.hook_reports(proto::AgentState::Idle);
    assert!(worker.waiting_on().is_some(), "the first answer ended a wait for the second");
}

#[test]
fn another_senders_message_read_first_does_not_end_the_wait() {
    let mut worker = worker_beside(&[("p2", "t2"), ("p3", "t3")]);
    expect(&mut worker.control, review_from("p3"), proto::Outcome::Done);
    reads(&mut worker);
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    worker.hook_reports(proto::AgentState::Idle);
    assert!(worker.waiting_on().is_some(), "p3 is not who the wait is on");
}

#[test]
fn a_wait_on_a_pane_ends_when_that_pane_closes() {
    let mut worker = worker_beside(&[("p2", "t2"), ("p3", "t3")]);
    assert_eq!(worker.declares_from("p2").outcome(), proto::Outcome::Done);
    worker.hook_reports(proto::AgentState::Idle);
    assert!(worker.waiting_on().is_some());

    crate::reopen::close(&mut worker.control, "p3");
    assert!(worker.waiting_on().is_some(), "an unrelated pane closing ended the wait");
    crate::reopen::close(&mut worker.control, "p2");
    worker.until_wait_over();
}
