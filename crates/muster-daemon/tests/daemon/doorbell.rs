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

    /// The lines the agent has read with a ring in them, whatever was typed around it.
    fn rings_heard(&self) -> Vec<String> {
        self.heard().into_iter().filter(|line| line.contains("[muster] ")).collect()
    }

    /// Types into the agent's pane as a person would, with a Return or without.
    fn type_in(&self, text: &str, enter: bool) {
        Input::connect(self.daemon.socket_path()).send(
            "p1",
            proto::input_event::Input::Send(proto::input_event::Send {
                text: text.to_string(),
                enter,
            }),
        );
    }

    fn until_shows(&mut self, text: &str) {
        until_text(&mut self.control, "p1", text);
    }

    fn until_rung(&self, times: usize) -> Vec<String> {
        until_some(&format!("the agent to be rung {times} time(s)"), || {
            let rung = self.rung();
            (rung.len() >= times).then_some(rung)
        })
    }

    /// Pauses or resumes a group, as the integrator.
    fn pausing(&mut self, group: &str, paused: bool) {
        let group = group.to_string();
        let asked = if paused {
            Asked::Pause(msg_request::Pause { group })
        } else {
            Asked::Resume(msg_request::Resume { group })
        };
        let caller = msg_request::Caller {
            as_name: Some("integrator".to_string()),
            ..msg_request::Caller::default()
        };
        let service =
            Service::Msg(proto::MsgRequest { caller: Some(caller), request: Some(asked) });
        expect(&mut self.control, service, proto::Outcome::Done);
    }

    fn post(&mut self, to: &str, body: &str) -> msg_answer::Posted {
        self.posting(to, body, false)
    }

    fn post_urgently(&mut self, to: &str, body: &str) -> msg_answer::Posted {
        self.posting(to, body, true)
    }

    fn posting(&mut self, to: &str, body: &str, urgent: bool) -> msg_answer::Posted {
        let asked = Asked::Post(msg_request::Post {
            body: body.to_string(),
            to: vec![to.to_string()],
            urgent,
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

/// Nor is Return pressed again once a person has typed into the pane, even when what they typed
/// has been taken back and the prompt holds the ring alone again. A letter taken back and words
/// the agent has yet to paint look the same from here, and a Return over the second sends them.
#[test]
fn a_ring_somebody_typed_at_is_not_pressed_again_even_once_it_is_taken_back() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent("p1");
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");

    // Just before the Return is due, a person types a letter and takes it back.
    std::thread::sleep(ANSWER.saturating_sub(Duration::from_secs(1)));
    agent.type_in("x", false);
    agent.type_in("\u{7f}", false);
    std::thread::sleep(ANSWER + QUIET + Duration::from_secs(2));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "Return pressed after typing");
}

/// Detection reads a new agent as idle before it has read its screen at all, for the few
/// seconds it gives an agent to draw. An agent that opens on a question - whether to trust a
/// folder, whether to update - is at a dialog through all of them, and a Return answers it. It
/// is rung only once its screen reads as its prompt.
#[test]
fn an_agent_that_opens_on_a_dialog_is_not_rung_until_its_prompt_shows() {
    let mut agent = Agent::to_come();
    agent.post("p1", "a brief");
    agent.daemon.run_agent_at_a_dialog("p1");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung at its opening dialog");

    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    agent.until_rung(1);
}

/// A menu opened from the prompt keeps the state it opened over, idle, and a Return there
/// picks an entry.
#[test]
fn an_agent_with_a_menu_open_is_not_rung() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("menu", true);
    agent.until_shows("PROBE-MENU");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung over its menu");

    agent.type_in("nomenu", true);
    agent.until_rung(1);
}

/// Something half typed into the prompt is a person's draft, however long ago they paused, and
/// a ring would be sent inside it. It waits until the prompt is empty.
#[test]
fn a_prompt_with_something_half_typed_in_it_is_not_rung() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("half typed", false);
    agent.until_shows("PROBE-PROMPT> half typed");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung into a draft");

    agent.type_in("", true);
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with("[muster] integrator+p1: 1 new"), "{rung:?}");
    assert!(agent.heard().contains(&"half typed".to_string()), "{:?}", agent.heard());
}

/// Return is pressed again only over the ring's own text, unsent. Once somebody has typed after
/// it, the prompt holds their words too, and a Return would send them.
#[test]
fn a_ring_left_in_a_prompt_somebody_then_typed_into_is_not_sent() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent("p1");
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");
    agent.type_in(" and more", false);

    std::thread::sleep(ANSWER + QUIET + Duration::from_secs(2));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "Return pressed over typed words");
}

/// The same, with the agent slow to paint what was typed after the ring: its prompt shows the
/// ring alone when Return is due again, and the words typed after it would go with a Return.
/// The daemon has seen them typed, which is enough to stop.
#[test]
fn a_ring_is_not_pressed_again_over_words_the_agent_has_not_painted_yet() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent_unechoed("p1");
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");
    agent.type_in(" and more", false);

    std::thread::sleep(ANSWER + QUIET + Duration::from_secs(2));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "Return pressed over typed words");
}

/// A suggestion drawn faint in an empty prompt is not something anybody typed.
#[test]
fn a_prompt_showing_a_faint_suggestion_is_empty() {
    let mut agent = Agent::to_come();
    agent.daemon.run_agent_with_a_suggestion("p1");
    agent.post("p1", "a brief");
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with("[muster] integrator+p1: 1 new"), "{rung:?}");
}

/// An agent that exits leaves its shell's prompt in the pane, and a ring waiting for it is not
/// typed there.
#[test]
fn a_ring_waiting_for_an_agent_that_exits_is_not_typed_into_its_shell() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    agent.post("p1", "a brief");
    agent.type_in("quit", true);
    std::thread::sleep(QUIET + Duration::from_secs(2));
    let shown = read_text(&mut agent.control, "p1", 0, 0).text;
    assert!(!shown.contains("[muster]"), "typed into the shell: {shown}");
}

/// A daemon that takes the panes over by a handoff does not know whether a wake still unread
/// was rung, so it rings it again, without waiting for a post or a change in the pane to prompt
/// it.
#[test]
fn a_daemon_that_took_over_rings_a_wake_still_unread() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "a brief");
    agent.until_rung(1);

    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    let rung = agent.until_rung(2);
    assert!(rung[1].starts_with("[muster] integrator+p1: 1 new"), "{rung:?}");
}

/// A paused group's posts ring nobody, and resuming it rings its agent once for everything it
/// has unread (MIP-4, section 8).
#[test]
fn a_paused_group_rings_nobody_and_resuming_it_rings_once() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "one");
    agent.until_rung(1);

    agent.pausing("integrator+p1", true);
    let posted = agent.post("p1", "two");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Paused);
    std::thread::sleep(QUIET + Duration::from_secs(2));
    assert_eq!(agent.rung().len(), 1, "rung while paused: {:?}", agent.rung());

    agent.pausing("integrator+p1", false);
    let rung = agent.until_rung(2);
    assert!(rung[1].contains("2 new"), "{rung:?}");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung().len(), 2, "rung more than once on resuming: {:?}", agent.rung());
}

/// The agent in `p1`, as its own hooks ask: a `join` or `wait` sent from inside its pane.
fn from_the_pane(asked: Asked) -> Service {
    let caller = msg_request::Caller { pane: Some("p1".to_string()), ..Default::default() };
    Service::Msg(proto::MsgRequest { caller: Some(caller), request: Some(asked) })
}

/// An agent whose hooks fetch its messages, at work, sent a message its hooks were counted on to
/// fetch: nothing is typed into its pane while it works.
fn at_work_with_a_message_its_hooks_took() -> Agent {
    let mut agent = Agent::in_a_pane();
    let join = Asked::Join(msg_request::Join {
        name: Some("worker".to_string()),
        group: Some("integrator+worker".to_string()),
        pull: true,
    });
    expect(&mut agent.control, from_the_pane(join), proto::Outcome::Done);
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    let posted = agent.post("worker", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Woken);
    agent
}

/// A turn that ends in an API error, or that a person stops with Esc, runs no `Stop` hook, so
/// nothing fetches what the hooks were counted on to fetch. Its agent is rung for it once idle.
#[test]
fn an_agent_whose_turn_ended_without_its_stop_hook_is_rung_for_what_its_hooks_took() {
    let agent = at_work_with_a_message_its_hooks_took();
    std::thread::sleep(QUIET);
    assert!(agent.rung().is_empty(), "rung at work: {:?}", agent.rung());

    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("still unread"), "{rung:?}");
}

/// A turn that ends as usual starts its `Stop` hook, whose wait is told what the hooks took,
/// whether it connects before the doorbell sees the agent idle or just after: nothing is typed.
#[test]
fn an_agent_whose_stop_hook_waits_is_told_there_and_not_rung() {
    let agent = at_work_with_a_message_its_hooks_took();
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    std::thread::sleep(Duration::from_millis(500));
    let mut waiting = agent.daemon.connect();
    let wait = Asked::Wait(msg_request::Wait { due: true, ..Default::default() });
    waiting.send(from_the_pane(wait));
    match waiting.next_message(Duration::from_secs(20)) {
        Some(proto::control_message::Message::Answer(proto::Answer {
            detail:
                Some(proto::answer::Detail::Msg(proto::MsgAnswer {
                    answer: Some(Answer::Notices(notices)),
                    ..
                })),
            ..
        })) => assert_eq!(notices.notices[0].group, "integrator+worker"),
        other => panic!("the Stop hook's wait was not told: {other:?}"),
    }
    std::thread::sleep(QUIET + Duration::from_secs(2));
    assert!(agent.rung().is_empty(), "rung as well: {:?}", agent.rung());
}

/// An urgent post rings an agent at work, at its prompt, where Claude Code queues what is typed
/// for the turn it is running; an ordinary post to the same agent still waits for it to be idle.
/// The urgent ring stands for the ordinary one waiting, so the agent is rung once.
#[test]
fn an_urgent_post_rings_an_agent_at_work_and_an_ordinary_one_waits_for_idle() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    let posted = agent.post("p1", "when you are done");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);
    assert_eq!(posted.reached[0].until(), msg_answer::Until::Idle);
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung(), Vec::<String>::new(), "an ordinary post rang an agent at work");

    let posted = agent.post_urgently("p1", "stop: the schema changed");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Woken);
    assert_eq!(posted.reached[0].activity(), msg_answer::Activity::Working);
    let rung = agent.until_rung(1);
    assert_eq!(
        rung,
        [
            "[muster] integrator+p1: 2 new (#4-5), 1 urgent, 2 to you, from integrator. Read it now, before you go on: muster msg read --group integrator+p1"
        ]
    );
    std::thread::sleep(ANSWER + QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung().len(), 1, "rung again: {:?}", agent.heard());
    assert_eq!(agent.pressed(), 0, "Return pressed again over a ring taken: {:?}", agent.heard());
}

/// Words somebody typed into the prompt of an agent at work are a draft: an urgent ring waits
/// until it is sent or cleared, as it would at an idle prompt.
#[test]
fn an_urgent_post_waits_out_a_draft_typed_while_the_agent_works() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.type_in("half typed", false);
    agent.until_shows("PROBE-PROMPT> half typed");
    std::thread::sleep(QUIET);

    let posted = agent.post_urgently("p1", "now");
    assert_eq!(posted.reached[0].until(), msg_answer::Until::Prompt);
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung into a draft");

    agent.type_in("", true);
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
    assert!(agent.heard().contains(&"half typed".to_string()), "{:?}", agent.heard());
}

/// An urgent post never answers a dialog: it waits until its agent is out of it, and rings it
/// at work if that is where the dialog left it.
#[test]
fn an_urgent_post_waits_out_a_dialog() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    std::thread::sleep(QUIET);

    let posted = agent.post_urgently("p1", "now");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);
    assert_eq!(posted.reached[0].until(), msg_answer::Until::Unblocked);
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung at a dialog");

    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
}

/// An agent at work may open a dialog after its prompt was read and before the ring's Return,
/// where Return would answer the dialog: the ring is typed without it, and the Return comes
/// only once the prompt is seen holding the ring alone, which a dialog drawn over it is not.
#[test]
fn a_dialog_opened_over_an_urgent_ring_before_its_return_is_never_answered() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    agent.until_shows("PROBE-PROMPT> [muster] integrator+p1");
    agent.daemon.block_agent_unasked();
    agent.until_shows("PROBE-STATE:BLOCKED");
    std::thread::sleep(ANSWER * 2);
    assert_eq!(agent.heard(), ["working"], "a Return reached the dialog");
}

/// An idle agent whose screen never stops moving - an animated statusline, a clock - is still rung:
/// waiting for half a second of stillness would wait forever. Its prompt, read empty just before,
/// is the guard then.
#[test]
fn an_idle_agent_whose_screen_keeps_moving_is_still_rung() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("animate", true);
    agent.until_shows("frame ");
    std::thread::sleep(QUIET);

    agent.post("p1", "hello");
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with("[muster] integrator+p1"), "{rung:?}");
    agent.type_in("still", true);
}

/// A ring typed at work has its Return about a second later, once the screen shows it: the
/// window a dialog can open in is that second, not the quiet period that the ring's own typing
/// would start.
#[test]
fn an_urgent_ring_at_work_is_returned_about_a_second_after_it_is_typed() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    agent.until_shows("PROBE-PROMPT> [muster] integrator+p1");
    let shown = Instant::now();
    agent.until_rung(1);
    let took = shown.elapsed();
    assert!(took < Duration::from_millis(2500), "the Return came {took:?} after the ring showed");
}

/// A dialog that stays open over an urgent ring past the doorbell's patience for Returns, and then
/// closes with the ring still unsent in the prompt: the ring is sent then, not left there for good,
/// which would leave the agent counted woken and never rung for the message again.
#[test]
fn an_urgent_ring_a_long_dialog_covered_is_sent_once_the_dialog_closes() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    agent.until_shows("PROBE-PROMPT> [muster] integrator+p1");
    agent.daemon.block_agent_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Blocked);
    // Past six Returns' worth of looks, five seconds apart.
    std::thread::sleep(ANSWER * 7);
    assert_eq!(agent.heard(), ["working"], "a Return reached the dialog");

    agent.daemon.unblock_agent_unasked();
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
}

/// A ring typed at work, then something typed into the pane before it was seen taken: the ring
/// is left alone, and the agent still counts as woken, so it is rung once more as it goes idle
/// with the message unread.
#[test]
fn an_agent_rung_at_work_whose_pane_is_typed_into_is_rung_again_as_it_goes_idle() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    agent.until_rung(1);
    agent.type_in("z", false);
    std::thread::sleep(ANSWER + Duration::from_secs(1));
    agent.type_in("idle", true);
    agent.daemon.until_agent("p1", proto::AgentState::Idle);
    let rung = agent.until_rung(2);
    assert!(rung[1].contains("still unread"), "{rung:?}");
}

/// Nor a menu opened over the prompt of an agent at work.
#[test]
fn an_urgent_post_waits_out_a_menu_opened_while_the_agent_works() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.type_in("menu", true);
    agent.until_shows("PROBE-MENU");
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung over a menu");

    agent.type_in("nomenu", true);
    agent.until_rung(1);
}
