//! The doorbell (MIP-4, section 6): a wake typed into the pane an agent runs in, as one line and
//! a Return, only once the pane allows - never while its agent is blocked, where the Return
//! would answer a dialog, and never within a few seconds of anything typed there. The agent is
//! the harness's fake, which paints the states detection reads and writes down every line it
//! reads, so what the daemon typed is on disk after the screen is painted over.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Input;
use proto::msg_answer::{self, Answer};
use proto::msg_request::{self, Request as Asked};
use proto::request::Service;

/// The daemon's quiet period: how long nothing may have been typed into a pane before it is
/// rung.
const QUIET: Duration = Duration::from_secs(3);

/// How long the daemon gives a rung agent to take the ring before pressing Return again.
const ANSWER: Duration = Duration::from_secs(5);

struct Agent {
    daemon: Daemon,
    control: Control,
}

impl Agent {
    /// A daemon with the fake agent idle in pane `p1`.
    fn in_a_pane() -> Agent {
        let agent = Agent::to_come();
        agent.daemon.run_agent("p1");
        agent
    }

    /// A daemon with pane `p1` at its shell, where no agent runs yet.
    fn to_come() -> Agent {
        let daemon = Daemon::start_detecting();
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        Agent { daemon, control }
    }

    fn heard_file(&self) -> PathBuf {
        self.daemon.root().join("home/fake-agent-heard")
    }

    /// The lines the agent has read that a doorbell typed.
    fn rung(&self) -> Vec<String> {
        std::fs::read_to_string(self.heard_file())
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("[muster] "))
            .map(str::to_string)
            .collect()
    }

    /// Everything the agent has read, in order: a Return pressed again is an empty line.
    fn heard(&self) -> Vec<String> {
        std::fs::read_to_string(self.heard_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn pressed(&self) -> usize {
        self.heard().iter().filter(|line| line.is_empty()).count()
    }

    fn until_rung(&self, times: usize) -> Vec<String> {
        until_some(&format!("the agent to be rung {times} time(s)"), || {
            let rung = self.rung();
            (rung.len() >= times).then_some(rung)
        })
    }

    fn post(&mut self, to: &str, body: &str) -> msg_answer::Posted {
        let asked = Asked::Post(msg_request::Post {
            body: body.to_string(),
            to: vec![to.to_string()],
            ..Default::default()
        });
        let caller = msg_request::Caller {
            as_name: Some("integrator".to_string()),
            ..msg_request::Caller::default()
        };
        let service =
            Service::Msg(proto::MsgRequest { caller: Some(caller), request: Some(asked) });
        let asked = expect(&mut self.control, service, proto::Outcome::Done);
        match &asked.answer.detail {
            Some(proto::answer::Detail::Msg(proto::MsgAnswer {
                answer: Some(Answer::Posted(posted)),
                ..
            })) => posted.clone(),
            other => panic!("expected a post's answer, got {other:?}"),
        }
    }
}

/// A blocked agent is at a dialog, and a Return would answer it. The message waits, and the
/// ring comes once the agent is idle and nobody has typed for the quiet period.
#[test]
fn a_blocked_agent_is_never_rung_and_is_rung_once_it_is_idle() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    std::thread::sleep(QUIET);

    // Addressed by its pane, though it never joined anything.
    let posted = agent.post("p1", "a brief");
    let reached = &posted.reached[0];
    assert_eq!(reached.name, "p1");
    assert_eq!(reached.reach(), msg_answer::Reach::Deferred);
    assert_eq!(reached.activity(), msg_answer::Activity::Blocked);

    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung(), Vec::<String>::new(), "a blocked pane was rung");

    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    agent.until_rung(1);
    // Going idle from the dialog came before the ring, so it is no reason to ring again.
    std::thread::sleep(QUIET + Duration::from_secs(1));
    let rung = agent.rung();
    assert_eq!(
        rung,
        [
            "[muster] integrator+p1: 1 new (#4), 1 to you, from integrator. Read: muster msg read --group integrator+p1"
        ]
    );
}

/// Something typed into a pane may be a person's prompt half written; the ring waits until
/// nothing has been typed for the quiet period.
#[test]
fn a_pane_typed_into_is_rung_only_once_it_has_been_quiet() {
    let mut agent = Agent::in_a_pane();
    let typed = Instant::now();
    Input::connect(agent.daemon.socket_path()).send(
        "p1",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: "a person typing".to_string(),
            enter: true,
        }),
    );
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);
    agent.until_rung(1);
    assert!(
        typed.elapsed() >= QUIET,
        "rung {:?} after something was typed, inside the quiet period",
        typed.elapsed()
    );
}

/// An agent that finishes its turn with the message still unread is woken once more, saying
/// so, and not a third time until it reads.
#[test]
fn an_agent_gone_idle_with_messages_unread_is_rung_once_more() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "a brief");
    agent.until_rung(1);

    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    let rung = agent.until_rung(2);
    assert!(rung[1].contains("still unread"), "{rung:?}");

    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung().len(), 2, "woken a third time: {:?}", agent.rung());
}

/// `muster pane new --run claude` and then a post to the pane it printed: the post comes before
/// detection has found the agent, and is rung once it has and the agent is idle (MIP-4, section
/// 14).
#[test]
fn a_pane_addressed_before_its_agent_is_found_is_rung_once_it_is() {
    let mut agent = Agent::to_come();
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);

    agent.daemon.run_agent("p1");
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with("[muster] integrator+p1: 1 new"), "{rung:?}");
}

/// Claude Code is found, and read as idle, before it reads its terminal as its prompt does: what
/// is typed while it starts fills the prompt, and the Return typed with it is dropped. A ring
/// that the agent neither acts on nor reads for is followed by another Return, which sends it.
#[test]
fn a_ring_typed_while_the_agent_starts_is_sent_by_a_later_return() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent("p1");
    agent.post("p1", "a brief");

    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with("[muster] integrator+p1: 1 new"), "{rung:?}");
}

/// A Return pressed again is a Return typed at the agent, under the ring's own guards, checked
/// again each time: an agent that turns blocked after its ring - at a permission prompt, say -
/// is pressed nothing, however long it stays there.
#[test]
fn a_return_is_never_pressed_again_at_an_agent_that_turned_blocked() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "a brief");
    agent.until_rung(1);
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);

    std::thread::sleep(ANSWER + QUIET + Duration::from_secs(2));
    assert_eq!(agent.pressed(), 0, "Return pressed at a blocked agent: {:?}", agent.heard());

    // Out of the dialog with the message unread, it is woken once more.
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    let rung = agent.until_rung(2);
    assert!(rung[1].contains("still unread"), "{rung:?}");
}

/// Nor is Return pressed again within the quiet period of something typed into the pane, where
/// it would send a person's half-written prompt.
#[test]
fn a_return_is_pressed_again_only_once_the_pane_has_been_quiet() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "a brief");
    agent.until_rung(1);

    // Just before the Return is due, a person types.
    std::thread::sleep(ANSWER.saturating_sub(Duration::from_secs(1)));
    let typed = Instant::now();
    Input::connect(agent.daemon.socket_path()).send(
        "p1",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: "a person typing".to_string(),
            enter: true,
        }),
    );
    until_some("a Return pressed again", || (agent.pressed() > 0).then_some(()));
    assert!(
        typed.elapsed() >= QUIET,
        "Return pressed {:?} after something was typed, inside the quiet period",
        typed.elapsed()
    );
    let heard = agent.heard();
    let person = heard.iter().position(|line| line == "a person typing");
    let pressed = heard.iter().position(String::is_empty);
    assert!(person < pressed, "Return pressed before the person typed: {heard:?}");
}
