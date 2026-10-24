//! The doorbell (MIP-4, section 6): a wake typed into the pane an agent runs in, as one line and
//! a Return, only once the pane allows - never while its agent is blocked, where the Return
//! would answer a dialog, and never within a few seconds of anything typed there. The agent is
//! the harness's fake, which paints the states detection reads and writes down every line it
//! reads, so what the daemon typed is on disk after the screen is painted over.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::persistence::{stop, until_restored, until_saved};
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

/// How long the daemon gives an agent woken by command to take the wake before typing it.
const COMMAND_ANSWER: Duration = Duration::from_secs(20);

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

    /// [`Agent::in_a_pane`], on a daemon with more in its environment.
    fn in_a_pane_with(environment: &[(&str, &str)]) -> Agent {
        let daemon = Daemon::start_detecting_with(environment);
        let mut control = daemon.connect();
        make(&mut control, create("p1", in_new_tab("t1")));
        daemon.run_agent("p1");
        Agent { daemon, control }
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
        self.type_in_pane("p1", text, enter);
    }

    fn type_in_pane(&self, pane: &str, text: &str, enter: bool) {
        Input::connect(self.daemon.socket_path()).send(
            pane,
            proto::input_event::Input::Send(proto::input_event::Send {
                text: text.to_string(),
                enter,
                ..Default::default()
            }),
        );
    }

    /// The group the doorbell's tests post to `pane` in: `integrator` and the word the pane was
    /// drawn, named as a pair's group is, in order.
    fn pair(&self, pane: &str) -> String {
        let word = snapshot(&mut self.daemon.connect())
            .panes
            .into_iter()
            .find(|record| record.pane == pane)
            .and_then(|record| record.callsign)
            .unwrap_or_else(|| panic!("{pane} has no callsign"));
        let mut pair = ["integrator".to_string(), word];
        pair.sort();
        pair.join("+")
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

    /// What `name` has not read yet, each message as `author: body`, which reading marks read.
    fn read_as(&mut self, name: &str) -> Vec<String> {
        self.reading(msg_request::Caller {
            as_name: Some(name.to_string()),
            ..msg_request::Caller::default()
        })
    }

    /// What the agent's prompt holds, by a read asking for it.
    fn prompt_holds(&mut self) -> Option<String> {
        let read = pane(proto::pane_request::Request::Read(proto::pane_request::Read {
            pane: "p1".to_string(),
            last: 1,
            prompt: true,
            ..Default::default()
        }));
        match expect(&mut self.control, read, proto::Outcome::Done).answer.detail {
            Some(proto::answer::Detail::Text(text)) => text.prompt,
            other => panic!("a read answered with {other:?}"),
        }
    }

    /// What the agent in p1 has not read yet, read as it reads from its own pane.
    fn read_in_its_pane(&mut self) -> Vec<String> {
        self.reading(msg_request::Caller {
            pane: Some("p1".to_string()),
            ..msg_request::Caller::default()
        })
    }

    fn reading(&mut self, caller: msg_request::Caller) -> Vec<String> {
        let service = Service::Msg(proto::MsgRequest {
            caller: Some(caller),
            request: Some(Asked::Read(msg_request::Read { group: None })),
        });
        let asked = expect(&mut self.control, service, proto::Outcome::Done);
        let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
            answer: Some(Answer::Entries(entries)),
            ..
        })) = &asked.answer.detail
        else {
            panic!("expected entries, got {:?}", asked.answer.detail);
        };
        let mut messages = Vec::new();
        for group in &entries.groups {
            for entry in &group.entries {
                if let Some(msg_answer::entry::What::Message(message)) = &entry.what {
                    messages.push(format!("{}: {}", message.author, message.body));
                }
            }
        }
        messages
    }

    /// What `who` says waits unread on each member, as `name: why`.
    fn unread(&mut self) -> Vec<String> {
        let service = Service::Msg(proto::MsgRequest {
            caller: Some(msg_request::Caller {
                as_name: Some("integrator".to_string()),
                ..msg_request::Caller::default()
            }),
            request: Some(Asked::Who(msg_request::Who::default())),
        });
        let asked = expect(&mut self.control, service, proto::Outcome::Done);
        let Some(proto::answer::Detail::Msg(proto::MsgAnswer {
            answer: Some(Answer::Members(members)),
            ..
        })) = &asked.answer.detail
        else {
            panic!("expected members, got {:?}", asked.answer.detail);
        };
        members
            .members
            .iter()
            .flat_map(|member| {
                member.unread.iter().map(|unread| format!("{}: {}", member.name, unread.why))
            })
            .collect()
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
        self.posting_as("integrator", to, body, urgent)
    }

    /// A post from `sender`, which puts it and the pane in a group of their own.
    fn posting_as(
        &mut self,
        sender: &str,
        to: &str,
        body: &str,
        urgent: bool,
    ) -> msg_answer::Posted {
        let asked = Asked::Post(msg_request::Post {
            body: body.to_string(),
            to: vec![to.to_string()],
            urgent,
            ..Default::default()
        });
        let caller = msg_request::Caller {
            as_name: Some(sender.to_string()),
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
        [format!(
            "[muster] {0}: 1 new (#4), 1 to you, from integrator. Read: muster msg read --group {0}",
            agent.pair("p1")
        )]
    );
}

/// A renamed pane is posted to by its callsign, which its row still shows in blue and which an
/// agent may have been told before the rename.
#[test]
fn a_renamed_pane_is_posted_to_by_its_callsign() {
    let mut agent = Agent::in_a_pane();
    let rename =
        proto::pane_request::Rename { pane: "p1".to_string(), label: Some("builder".to_string()) };
    let renamed = pane(proto::pane_request::Request::Rename(rename));
    expect(&mut agent.control, renamed, proto::Outcome::Done);
    let record = snapshot(&mut agent.control).panes.into_iter().find(|record| record.pane == "p1");
    let callsign = record.and_then(|record| record.callsign).expect("p1 was drawn a callsign");

    let posted = agent.post(&callsign, "a brief");
    assert_eq!(posted.reached[0].name, "p1", "{posted:?}");
    agent.until_rung(1);
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
            ..Default::default()
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

/// An agent rung and still idle with what it was rung for unread will not read a later post by
/// itself: its turn ended without reading, or `/new` threw the ring away with its session. The
/// later post rings it, saying everything that waits.
#[test]
fn a_post_rings_an_idle_agent_woken_already() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "a brief");
    agent.until_rung(1);
    // Once the doorbell has stopped watching the ring, nothing more is on its way.
    until_some("the ring to be settled", || {
        agent.unread().iter().any(|why| why.contains("neither read it nor gone to work")).then_some(())
    });

    let posted = agent.post("p1", "the brief again");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Woken);
    let rung = agent.until_rung(2);
    assert!(rung[1].contains("2 new"), "{rung:?}");
}

/// A post that comes while a ring is still on its way to an idle agent - typed, and not yet seen
/// taken - leaves it to that ring rather than ringing a second time.
#[test]
fn a_post_while_a_ring_is_on_its_way_rings_nothing_more() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent("p1");
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");

    let posted = agent.post("p1", "and another");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Woken);
    agent.until_rung(1);
    std::thread::sleep(QUIET + ANSWER + Duration::from_secs(2));
    assert_eq!(agent.rung().len(), 1, "rung twice: {:?}", agent.rung());
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
    assert!(rung[0].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
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
    assert!(rung[0].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
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
/// picks an entry. The post says the menu is what holds its wake.
#[test]
fn an_agent_with_a_menu_open_is_not_rung() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("menu", true);
    agent.until_shows("PROBE-MENU");
    std::thread::sleep(QUIET);
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].until(), msg_answer::Until::Prompt);
    assert_eq!(
        posted.reached[0].held.as_deref(),
        Some("a menu opened from its prompt covers its prompt")
    );
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung over its menu");

    agent.type_in("nomenu", true);
    agent.until_rung(1);
}

/// Something half typed into the prompt is a person's draft, however long ago they paused, and
/// a ring would be sent inside it. It waits until the prompt is empty, and the post says so.
#[test]
fn a_prompt_with_something_half_typed_in_it_is_not_rung() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("half typed", false);
    agent.until_shows("PROBE-PROMPT> half typed");
    std::thread::sleep(QUIET);
    let posted = agent.post("p1", "a brief");
    assert_eq!(
        posted.reached[0].held.as_deref(),
        Some("its prompt holds something typed and not sent")
    );
    assert_eq!(
        agent.unread(),
        ["p1: a wake for it is held because its prompt holds something typed and not sent"]
    );
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rings_heard(), Vec::<String>::new(), "rung into a draft");

    agent.type_in("", true);
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
    assert!(agent.heard().contains(&"half typed".to_string()), "{:?}", agent.heard());
}

/// A read asking for the prompt says what it holds, as the doorbell reads it, and nothing once
/// it is empty: what `pane send` asks before typing text, so as not to add to a draft.
#[test]
fn a_read_says_what_the_agents_prompt_holds() {
    let mut agent = Agent::in_a_pane();
    assert_eq!(agent.prompt_holds(), None, "an empty prompt");

    agent.type_in("half typed", false);
    agent.until_shows("PROBE-PROMPT> half typed");
    let held = until_some("the prompt to be read holding the draft", || agent.prompt_holds());
    assert_eq!(held, "half typed");
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
    assert!(rung[0].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
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
    assert!(rung[1].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
}

/// A paused group's posts ring nobody, and resuming it rings its agent once for everything it
/// has unread (MIP-4, section 8).
#[test]
fn a_paused_group_rings_nobody_and_resuming_it_rings_once() {
    let mut agent = Agent::in_a_pane();
    agent.post("p1", "one");
    agent.until_rung(1);

    agent.pausing(&agent.pair("p1"), true);
    let posted = agent.post("p1", "two");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Paused);
    std::thread::sleep(QUIET + Duration::from_secs(2));
    assert_eq!(agent.rung().len(), 1, "rung while paused: {:?}", agent.rung());

    agent.pausing(&agent.pair("p1"), false);
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
        [format!(
            "[muster] {0}: 2 new (#4-5), 1 urgent, 2 to you, from integrator. Read it now, before you go on: muster msg read --group {0}",
            agent.pair("p1")
        )]
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
    agent.until_shows(&format!("PROBE-PROMPT> [muster] {0}", agent.pair("p1")));
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
    assert!(rung[0].starts_with(&format!("[muster] {0}", agent.pair("p1"))), "{rung:?}");
    agent.type_in("still", true);
}

/// A ring whose Return was dropped, in a pane whose screen never stops moving, still has Return
/// pressed again: the stillness the first ring waited out bounded is bounded for the presses too.
#[test]
fn a_ring_left_unsent_in_a_pane_that_keeps_moving_is_pressed_again() {
    let mut agent = Agent::to_come();
    agent.daemon.run_starting_agent_animated("p1");
    agent.until_shows("frame ");
    std::thread::sleep(QUIET);

    agent.post("p1", "hello");
    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with(&format!("[muster] {0}", agent.pair("p1"))), "{rung:?}");
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
    agent.until_shows(&format!("PROBE-PROMPT> [muster] {0}", agent.pair("p1")));
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
    agent.until_shows(&format!("PROBE-PROMPT> [muster] {0}", agent.pair("p1")));
    agent.daemon.block_agent_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Blocked);
    // Past six Returns' worth of looks, five seconds apart.
    std::thread::sleep(ANSWER * 7);
    assert_eq!(agent.heard(), ["working"], "a Return reached the dialog");

    agent.daemon.unblock_agent_unasked();
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
}

/// A ring typed at work whose message its agent reads by itself before the ring's Return - its
/// prompt hidden meanwhile, here by a dialog - is taken back once the prompt shows again: sent,
/// it would wake the agent for nothing, and left, whatever is typed next would be sent with it.
#[test]
fn an_urgent_ring_read_before_its_return_is_taken_back_from_the_prompt() {
    let mut agent = Agent::in_a_pane();
    let mut logging = following(&agent.daemon);
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(QUIET);

    agent.post_urgently("p1", "now");
    agent.until_shows(&format!("PROBE-PROMPT> [muster] {0}", agent.pair("p1")));
    agent.daemon.block_agent_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Blocked);
    let covered = r#""event":"msg.ring.covered""#;
    let logged = logging.logged_until(covered, Duration::from_secs(10));
    let line = logged.iter().find(|line| line.line.contains(covered));
    let line = line.unwrap_or_else(|| panic!("{logged:?}"));
    // What covered it is in the record by the rule that read the screen and the screen's shape,
    // never its text.
    assert!(line.line.contains(r#""rule":""#), "{}", line.line);
    assert!(!line.line.contains(r#""rule":"""#), "{}", line.line);
    assert!(!line.line.contains(r#""shape":"""#), "{}", line.line);
    assert!(!line.line.contains("PROBE-PROMPT"), "{}", line.line);
    assert_eq!(agent.read_in_its_pane(), ["integrator: now"]);

    agent.daemon.unblock_agent_unasked();
    let taken_back = r#""event":"msg.ring.taken_back""#;
    let logged = logging.logged_until(taken_back, Duration::from_secs(20));
    assert!(logged.iter().any(|line| line.line.contains(taken_back)), "{logged:?}");
    // The fake agent's prompt is its terminal's own line editing, whose echo of a wrapped line
    // erased is not a screen worth reading: what it reads next says what the prompt held.
    agent.type_in("next", true);
    until_some("the agent to read what was typed next", || {
        agent.heard().iter().any(|line| line.ends_with("next")).then_some(())
    });
    assert_eq!(agent.heard(), ["working", "next"], "the ring was sent, or left in the prompt");
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

impl Agent {
    /// The agent's hooks reporting its session's id, as Codex's `SessionStart` does.
    fn reports_session(&mut self, id: &str) {
        let report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            session_id: Some(id.to_string()),
            ..Default::default()
        };
        let asked = self.control.ask(pane(proto::pane_request::Request::Report(report)));
        assert_eq!(asked.outcome(), proto::Outcome::Done, "{}", asked.answer.reason);
    }

    /// What the fake agent's `[session] wake` command was run with, a line per run.
    fn woken(&self) -> Vec<String> {
        std::fs::read_to_string(self.daemon.root().join("home/fake-agent-woken"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// A session whose agent reported its id is woken by the command its manifest names, as Codex is
/// by `codex queue`: nothing is typed into the pane, so a draft there neither holds the wake back
/// nor is touched.
#[test]
fn a_session_with_its_id_reported_is_woken_by_its_command_without_typing() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("019a-s1");
    agent.type_in("half typed", false);
    agent.post("p1", "a brief");
    let woken = until_some("the wake command to run", || {
        let woken = agent.woken();
        (!woken.is_empty()).then_some(woken)
    });
    assert_eq!(
        woken,
        [format!(
            "019a-s1|[muster] {0}: 1 new (#4), 1 to you, from integrator. Read: muster msg read --group {0}",
            agent.pair("p1")
        )]
    );
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung(), Vec::<String>::new(), "the wake was typed as well");
    assert_eq!(agent.woken().len(), 1);
}

/// A post to an agent that could be rung at once, quiet and at an empty prompt, is not typed
/// into its pane either: the post hands it to the command just the same.
#[test]
fn a_quiet_session_with_its_id_reported_is_woken_by_its_command_from_the_post() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("019a-s1");
    std::thread::sleep(QUIET + Duration::from_secs(1));
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Woken);
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    std::thread::sleep(QUIET);
    assert_eq!(agent.rung(), Vec::<String>::new(), "the wake was typed as well");
}

/// A wait the agent declared outlasts the turn a wake handed over by command starts, as it does
/// one typed: Codex's prompt hook says no wait for a queued turn too, and nothing was typed.
/// Here the hook reports the turn before the screen shows it.
#[test]
fn a_declared_wait_outlasts_a_turn_its_wake_command_started() {
    declared_wait_through_a_command_wake(false);
}

/// The same with the screen first: the fake's wake command puts it to work, and the pane reads
/// working before the prompt hook reports, as a loaded machine or a quick Codex orders them.
#[test]
fn a_declared_wait_outlasts_a_command_wakes_turn_the_screen_shows_first() {
    declared_wait_through_a_command_wake(true);
}

fn declared_wait_through_a_command_wake(screen_first: bool) {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("019a-s1");
    let report = |state: Option<proto::AgentState>, waiting: &str| {
        let mut report = proto::pane_request::Report {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            waiting: Some(waiting.to_string()),
            ..Default::default()
        };
        if let Some(state) = state {
            report.set_state(state);
        }
        pane(proto::pane_request::Request::Report(report))
    };
    let waiting_on = |agent: &mut Agent| {
        let records = snapshot(&mut agent.control).panes;
        let record = records.into_iter().find(|record| record.pane == "p1");
        record.and_then(|record| record.facts?.waiting)
    };
    expect(&mut agent.control, report(None, "the full gate"), proto::Outcome::Done);

    agent.post("p1", "a brief");
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    if screen_first {
        agent.daemon.until_agent("p1", proto::AgentState::Working);
    }
    expect(&mut agent.control, report(Some(proto::AgentState::Working), ""), proto::Outcome::Done);
    expect(&mut agent.control, report(Some(proto::AgentState::Idle), ""), proto::Outcome::Done);
    assert_eq!(waiting_on(&mut agent).as_deref(), Some("the full gate"));
}

/// The command is used only where a ring would reach the agent idle: a blocked agent is at a
/// dialog, where Codex stores a queued message and never submits it.
#[test]
fn a_blocked_session_is_not_woken_by_its_command_until_it_is_idle() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("019a-s1");
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    agent.post("p1", "a brief");
    std::thread::sleep(QUIET);
    assert_eq!(agent.woken(), Vec::<String>::new(), "woken at a dialog");
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    assert_eq!(agent.rung(), Vec::<String>::new());
}

/// A command that fails leaves the wake to the doorbell, which types it once the prompt allows,
/// and is not tried again for that session.
#[test]
fn a_wake_command_that_fails_is_typed_instead() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("gone");
    agent.post("p1", "a brief");
    agent.until_rung(1);
    assert_eq!(agent.woken(), Vec::<String>::new());
}

/// `codex queue` stores a message for a session whose Codex has exited and says it queued it, as
/// a `codex exec` run in the pane leaves behind: a command wake whose agent neither goes to work
/// nor reads is typed instead, and the command is not run for that session again.
#[test]
fn a_command_wake_nobody_takes_is_typed_instead() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("dead");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    // The daemon gives a command wake 20 seconds, past Codex's queue poll, before typing it.
    muster_harness::until_within(
        "the wake to be typed instead",
        COMMAND_ANSWER + muster_harness::PATIENCE,
        || !agent.rung().is_empty(),
        || format!("heard: {:?}", agent.heard()),
    );
    assert_eq!(agent.rung().len(), 1);
    assert_eq!(agent.woken().len(), 1, "the command was run again");
}

/// A Codex running its session without the shared app-server polls its queue, and starts the turn
/// for a stored wake up to ten seconds later. That wake is not typed as well: typed, it starts a
/// turn of its own, and Codex wakes the agent again with the stored copy as that turn ends, for a
/// message it has read and answered (docs/observations/codex-0.163.0.md).
#[test]
fn a_command_wake_its_agent_takes_late_is_not_typed_as_well() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("late");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    agent.until_shows("PROBE-STATE:WORKING");
    assert_eq!(agent.read_in_its_pane(), ["integrator: a brief"]);
    agent.type_in("idle", true);
    agent.until_shows("PROBE-STATE:IDLE");
    std::thread::sleep(QUIET + ANSWER);
    assert_eq!(agent.rung(), Vec::<String>::new(), "the stored wake was typed as well");
    assert_eq!(agent.woken().len(), 1);
}

/// A command that outlasts the daemon's patience - a login shell slow to start on a loaded machine -
/// is given up, and that wake typed instead rather than tried by command again.
#[test]
fn a_stalled_wake_command_is_typed_instead() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("slow");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    agent.until_rung(1);
    assert_eq!(agent.woken(), ["slow"], "the command was run again for the same wake");
}

/// A command slow to answer holds only its own pane: another pane's ring, due while it runs,
/// is not kept waiting for it.
#[test]
fn a_slow_wake_command_holds_no_other_panes_ring() {
    let mut agent = Agent::in_a_pane();
    make(&mut agent.control, create("p2", in_new_tab("t2")));
    agent.daemon.run_agent("p2");
    agent.reports_session("slow");
    std::thread::sleep(QUIET);

    // p2's ring waits out the quiet this starts, and falls due while p1's command runs.
    agent.type_in_pane("p2", "idle", true);
    std::thread::sleep(Duration::from_secs(2));
    agent.post("p1", "a brief");
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    let started = Instant::now();
    agent.post("p2", "another");
    until_some("p2 to be rung", || {
        agent.rung().iter().any(|rung| rung.contains(&agent.pair("p2"))).then_some(())
    });
    let waited = started.elapsed();
    assert!(waited < Duration::from_secs(3), "p2 was rung {waited:?} after p1's command started");
}

/// Two groups with something unread for one agent are two wakes for one pane, due in one look. Its
/// command runs for one at a time: two at once would let the first's end free the pane while the
/// second still ran, and a line could be typed into it then.
#[test]
fn a_pane_due_two_wakes_runs_one_wake_command_at_a_time() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("slow");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.post("p1", "a brief");
    agent.posting_as("reviewer", "p1", "a review", false);
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(agent.woken(), ["slow"], "a second command ran beside the first");
}

/// Codex reports its session's id only as the session starts, so a daemon taking the pane over
/// could not learn it again: it is handed over with the pane.
#[test]
fn a_reported_session_id_is_handed_over_with_its_pane() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("019a-s1");
    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    agent.control = agent.daemon.connect();
    agent.post("p1", "a brief");
    until_some("the wake command to run", || {
        agent.woken().iter().any(|woken| woken.starts_with("019a-s1|")).then_some(())
    });
}

/// A wake waiting on an agent at work outlives a daemon restart: the restarted daemon's first
/// look comes before its restore has brought the pane back, and holds the wake for the pane
/// rather than dropping it as closed, so the resumed agent is woken once it is idle.
#[test]
fn a_wake_waiting_across_a_restart_reaches_the_resumed_agent() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("s-1");
    until_saved(&agent.daemon, "s-1");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);

    stop(&mut agent.daemon, &mut agent.control);
    agent.daemon.restart();
    agent.control = agent.daemon.connect();
    until_restored(&mut agent.control, 1);
    until_some("the resumed agent to be woken", || {
        let woken = agent.woken().iter().any(|woken| woken.starts_with("s-1|"));
        (woken || !agent.rung().is_empty()).then_some(())
    });
    let log = std::fs::read_to_string(agent.daemon.root().join("daemon.log")).unwrap_or_default();
    assert!(!log.contains(r#""event":"msg.ring.dropped""#), "{log}");
}

/// A wake waiting for an agent that is restarted in its pane rings the agent resumed there, as
/// across a daemon's restart: between the one ending and the other being found, the pane has
/// an agent to come rather than none.
#[test]
fn a_wake_waiting_across_a_pane_restart_reaches_the_resumed_agent() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_AGENT_TO_COME_MS", "2000")]);
    // Past the time a new pane has an agent to come, so that what holds the wake is the restart.
    std::thread::sleep(Duration::from_millis(2500));
    agent.reports_session("s-1");
    until_saved(&agent.daemon, "s-1");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred);

    let restart = proto::pane_request::Restart { pane: "p1".to_string(), ..Default::default() };
    let restart = pane(proto::pane_request::Request::Restart(restart));
    expect(&mut agent.control, restart, proto::Outcome::Done);
    until_some("the resumed agent to be woken", || {
        let woken = agent.woken().iter().any(|woken| woken.starts_with("s-1|"));
        (woken || !agent.rung().is_empty()).then_some(())
    });
    let log = std::fs::read_to_string(agent.daemon.root().join("daemon.log")).unwrap_or_default();
    assert!(!log.contains(r#""event":"msg.ring.dropped""#), "{log}");
}

/// A post while its agent is still ending waits for the agent resuming in its place: the pane
/// refuses what would be typed into it meanwhile, which is no reason to tell the poster that
/// nobody heard.
#[test]
fn a_post_while_its_agent_restarts_waits_for_the_resumed_agent() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_RESTART_GRACE_MS", "2000")]);
    agent.reports_session("s-1");
    until_saved(&agent.daemon, "s-1");
    agent.daemon.type_into("p1", "stubborn");
    std::thread::sleep(Duration::from_millis(300));

    let mut restarting = agent.daemon.connect();
    let restart = proto::pane_request::Restart { pane: "p1".to_string(), ..Default::default() };
    let restart = std::thread::spawn(move || {
        let restart = pane(proto::pane_request::Request::Restart(restart));
        expect(&mut restarting, restart, proto::Outcome::Done);
    });
    let log = agent.daemon.root().join("daemon.log");
    until("the restart to begin", || written(&log).contains("daemon.pane.restarting"), ());
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].reach(), msg_answer::Reach::Deferred, "{posted:?}");

    restart.join().expect("the restart answered");
    until_some("the resumed agent to be woken", || {
        let woken = agent.woken().iter().any(|woken| woken.starts_with("s-1|"));
        (woken || !agent.rung().is_empty()).then_some(())
    });
}

/// A pane's record says its program has stopped reading, once it has.
fn until_stopped_reading(agent: &mut Agent) {
    until_some("p1's program to be published as having stopped reading", || {
        snapshot(&mut agent.control)
            .panes
            .into_iter()
            .find(|record| record.pane == "p1")
            .and_then(|record| record.unread_input)
    });
}

/// A connection the daemon's log streams to from its first line.
fn following(daemon: &Daemon) -> Control {
    let mut logging = daemon.connect();
    let follow = proto::session_request::Request::FollowLog(proto::session_request::FollowLog {
        after: None,
    });
    expect(&mut logging, session(follow), proto::Outcome::Done);
    logging
}

/// An agent whose program has stopped reading is not rung: the ring would wait in its terminal
/// with what was already sent. The post says so, and the agent is rung once it reads again.
#[test]
fn an_agent_that_stopped_reading_is_rung_once_it_reads_again() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("deaf 14", true);
    // Read, and so the evidence that this program reads, before anything waits unread.
    std::thread::sleep(Duration::from_secs(2));
    agent.type_in("are you there", true);
    until_stopped_reading(&mut agent);

    let posted = agent.post("p1", "a brief");
    let reached = &posted.reached[0];
    assert_eq!(reached.reach(), msg_answer::Reach::Deferred);
    assert_eq!(reached.activity(), msg_answer::Activity::Stuck);
    assert_eq!(reached.until(), msg_answer::Until::Reading);
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert_eq!(agent.rung(), Vec::<String>::new(), "a pane that is not reading was rung");

    let rung = agent.until_rung(1);
    assert!(rung[0].starts_with(&format!("[muster] {0}: 1 new", agent.pair("p1"))), "{rung:?}");
}

/// A ring typed into an agent whose program then stops reading is not taken, and not pressed
/// again: it ends, saying why, rather than being counted as woken for as long as the agent sits
/// there - which is how a director came to wait on an agent that would never answer.
#[test]
fn a_ring_an_agent_stops_reading_before_taking_ends() {
    let mut agent = Agent::in_a_pane();
    let mut logging = following(&agent.daemon);
    agent.type_in("deaf 20", true);
    let posted = agent.post("p1", "a brief");
    assert_eq!(posted.reached[0].activity(), msg_answer::Activity::Idle, "not stopped yet");

    let ended = r#""event":"msg.ring.ended""#;
    let logged = logging.logged_until(ended, Duration::from_secs(30));
    let line = logged
        .iter()
        .find(|line| line.line.contains(ended))
        .unwrap_or_else(|| panic!("the ring never ended"));
    assert!(line.line.contains("its program has stopped reading its input"), "{}", line.line);
    assert_eq!(agent.pressed(), 0, "Return was pressed again into a pane that was not reading");
}

/// A message that waits on an agent whose program stopped reading holds its group up. Once it
/// has for long enough, Muster tells whoever posted it why it has not answered, in the group,
/// once, and records the stall in the run log with the whole group as it stood.
#[test]
fn a_group_held_up_on_an_agent_that_stopped_reading_is_told_why_once() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "12000")]);
    let mut logging = following(&agent.daemon);
    agent.type_in("deaf 40", true);
    std::thread::sleep(Duration::from_secs(2));
    agent.post("p1", "a brief");

    let stalled = r#""event":"msg.group.stalled""#;
    let logged = logging.logged_until(stalled, Duration::from_secs(40));
    let line = logged
        .iter()
        .find(|line| line.line.contains(stalled))
        .unwrap_or_else(|| panic!("the stall was never recorded"));
    for said in [r#""member":"p1""#, r#""activity":"stuck""#, r#""unlock":"tell""#] {
        assert!(line.line.contains(said), "{said} in {}", line.line);
    }
    assert!(line.line.contains("integrator: not in a pane here"), "{}", line.line);

    let told = r#""event":"msg.group.told""#;
    logging.logged_until(told, Duration::from_secs(10));
    let read = agent.read_as("integrator");
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].starts_with("@muster: ")
            && read[0].contains(" (p1) has not read #4, from integrator")
            && read[0].contains("stopped reading its input"),
        "{read:?}"
    );

    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(agent.read_as("integrator"), Vec::<String>::new(), "told twice");
}

/// A wake held by what its pane shows is explained by that, and not by a guess: a menu over the
/// agent's prompt, in a pane its tab's zoom hides behind another.
#[test]
fn a_group_held_up_on_a_wake_a_menu_holds_is_told_so() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "12000")]);
    make(&mut agent.control, create("p2", beside("p1", proto::Side::Right)));
    let zoom = proto::pane_request::Zoom { pane: "p2".to_string(), zoomed: true };
    expect(
        &mut agent.control,
        pane(proto::pane_request::Request::Zoom(zoom)),
        proto::Outcome::Done,
    );
    agent.type_in("menu", true);
    agent.until_shows("PROBE-MENU");
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");

    let stalled = r#""event":"msg.group.stalled""#;
    let logged = logging.logged_until(stalled, Duration::from_secs(40));
    let line = logged
        .iter()
        .find(|line| line.line.contains(stalled))
        .unwrap_or_else(|| panic!("the stall was never recorded"));
    for said in [r#""held":"covered""#, r#""zoomed_on":"p2""#] {
        assert!(line.line.contains(said), "{said} in {}", line.line);
    }

    logging.logged_until(r#""event":"msg.group.told""#, Duration::from_secs(10));
    let read = agent.read_as("integrator");
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(read[0].contains("a menu opened from its prompt covers its prompt"), "{read:?}");
    assert!(read[0].contains("its tab is zoomed on "), "{read:?}");
    assert!(read[0].contains("(p2), which hides it"), "{read:?}");
}

/// A post that fails to trigger the agent it was for - rung, the ring read, and nothing done -
/// holds its group up too, and whoever posted it is told so, once.
#[test]
fn a_group_held_up_on_an_agent_that_ignored_its_ring_is_told_why() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "12000")]);
    agent.post("p1", "a brief");
    agent.until_rung(1);

    let mut logging = following(&agent.daemon);
    logging.logged_until(r#""event":"msg.group.told""#, Duration::from_secs(40));
    let read = agent.read_as("integrator");
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(read[0].contains("it was rung, and has neither read it nor gone to work"), "{read:?}");
}

/// A helper that blocks tells the pane that made it, once, in a message from Muster in a group
/// made for the two (MIP-7, section 4): a helper is quiet, and its maker is who can answer.
#[test]
fn a_helper_at_a_dialog_tells_its_maker_once() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);

    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    let read = agent.read_in_its_pane();
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].starts_with("@muster: ") && read[0].contains(" (p2) is blocked at a dialog: "),
        "{read:?}"
    );

    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "told twice");
}

/// A helper that blocks while its maker is at work is held until the maker can hear it, and told
/// then while it is still blocked: a post to a maker at work would wait for its turn to end
/// anyway. The advice names the helper by its callsign, which no other pane can hold.
#[test]
fn a_helper_blocked_while_its_maker_works_is_told_once_the_maker_is_free() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);

    logging.logged_until(r#""event":"msg.helper.held""#, Duration::from_secs(20));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "told a busy maker");

    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    let read = agent.read_in_its_pane();
    assert_eq!(read.len(), 1, "{read:?}");
    let callsign = snapshot(&mut agent.control)
        .panes
        .into_iter()
        .find(|record| record.pane == "p2")
        .and_then(|record| record.callsign)
        .expect("p2 was drawn a callsign");
    assert!(read[0].contains(&format!("{callsign} (p2) is blocked at a dialog: ")), "{read:?}");
    assert!(read[0].contains(&format!("--pane {callsign}`")), "{read:?}");
}

/// A helper's dialog answered while its maker was at work, as a maker approving its helpers'
/// plans answers each in the one turn, is never told: by the time the maker could read the
/// notice it would be stale.
#[test]
fn a_helper_answered_while_its_maker_worked_is_not_told() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);
    logging.logged_until(r#""event":"msg.helper.held""#, Duration::from_secs(20));

    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p1", proto::AgentState::Idle);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "told of a dialog answered");
}

/// A helper that blocks before reading its maker's task holds their group up too, and the maker
/// hears of it once: the maker watch tells it, and the stall watch leaves the maker to that.
#[test]
fn a_helper_blocked_on_its_makers_task_tells_the_maker_once() {
    let mut agent = Agent::in_a_pane_with(&[
        ("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000"),
        ("MUSTER_DAEMON_STALL_MS", "4000"),
    ]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    let task = Service::Msg(proto::MsgRequest {
        caller: Some(msg_request::Caller {
            pane: Some("p1".to_string()),
            ..msg_request::Caller::default()
        }),
        request: Some(Asked::Post(msg_request::Post {
            body: "a task".to_string(),
            to: vec!["p2".to_string()],
            ..Default::default()
        })),
    });
    expect(&mut agent.control, task, proto::Outcome::Done);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);

    let logged = logging.logged_until(r#""event":"msg.group.stalled""#, Duration::from_secs(30));
    let stalled = logged.iter().find(|line| line.line.contains("msg.group.stalled"));
    assert!(stalled.is_some_and(|line| line.line.contains(r#""unlock":"nothing""#)), "{logged:?}");
    std::thread::sleep(Duration::from_secs(2));
    let read = agent.read_in_its_pane();
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(read[0].contains(" (p2) is blocked at a dialog: "), "{read:?}");
}

/// A helper still blocked as a newer daemon takes its pane over has told its maker already, and
/// the new daemon does not tell it again.
#[test]
fn a_helper_blocked_across_a_handoff_tells_its_maker_once() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);
    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    assert_eq!(agent.read_in_its_pane().len(), 1);

    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    agent.control = agent.daemon.connect();
    until_some("the helper to read blocked on the new daemon", || {
        let panes = snapshot(&mut agent.control).panes;
        let helper = panes.into_iter().find(|pane| pane.pane == "p2")?;
        (helper.agent_state() == proto::AgentState::Blocked).then_some(())
    });
    // Any change in an agent wakes the doorbell's thread, which may otherwise sleep a minute.
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "told again");
}

/// A maker that closes leaves its helpers running: each stops being quiet, since nobody is left
/// to hear from it, and the person is told once, by `@muster` (MIP-7, section 6).
#[test]
fn a_closed_makers_helpers_stop_being_quiet_and_the_person_hears_once() {
    let mut agent = Agent::in_a_pane();
    let mut logging = following(&agent.daemon);
    let quiet_helper = proto::pane_request::Create {
        quiet: true,
        ..helper("p2", "p1", among_helpers("t2", "p1's helpers"))
    };
    make(&mut agent.control, quiet_helper);
    agent.daemon.run_agent("p2");
    let mut window = agent.daemon.connect();
    expect(&mut window, attending_request(), proto::Outcome::Done);

    expect(&mut agent.control, close_request("p1"), proto::Outcome::Done);
    logging.logged_until(r#""event":"msg.helper.orphans_told""#, Duration::from_secs(20));
    // The windows are handed Muster's words, for the banner to say rather than a count.
    let told = loop {
        if let Some(proto::event::Event::HumanNotice(told)) = window.next_event().event
            && told.count == 1
        {
            break told;
        }
    };
    assert!(told.said.contains(" closed; its helper ") && told.said.contains("--helpers-of p1"));
    let helper = snapshot(&mut agent.control).panes.into_iter().find(|pane| pane.pane == "p2");
    assert!(helper.is_some_and(|helper| !helper.quiet && helper.maker.as_deref() == Some("p1")));

    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].starts_with("@muster: ")
            && read[0].contains(" closed; its helper ")
            && read[0].contains("is still running, and is no longer quiet"),
        "{read:?}"
    );
}

/// A maker that closes together with its helpers, as a tab holding them all does, leaves nothing
/// running, so the person is told nothing.
#[test]
fn a_maker_closing_with_its_helpers_tells_nobody() {
    let mut agent = Agent::in_a_pane();
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", beside("p1", proto::Side::Right)));
    agent.daemon.run_agent("p2");

    let close = tab(proto::tab_request::Request::Close(proto::tab_request::Close {
        tab: "t1".to_string(),
    }));
    expect(&mut agent.control, close, proto::Outcome::Done);
    logging.logged_until(r#""event":"msg.helper.orphans_closed""#, Duration::from_secs(20));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new());
}

/// A daemon that stops closes every maker with its helpers, and both come back from its state
/// file, so no maker counts as closed.
#[test]
fn a_daemon_stopping_closes_no_maker() {
    let mut agent = Agent::in_a_pane();
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));

    stop(&mut agent.daemon, &mut agent.control);
    let log = std::fs::read_to_string(agent.daemon.root().join("daemon.log")).unwrap_or_default();
    assert!(log.contains(r#""event":"daemon.stopped""#), "{log}");
    assert!(!log.contains(r#""event":"daemon.maker.closed""#), "{log}");
}

/// A window that saw a maker on another machine close says so (MIP-7, section 6): its helpers
/// here stop being quiet and are marked, the person is told once, and saying it again changes
/// nothing. A maker open here has not closed.
#[test]
fn a_maker_closed_on_another_machine_frees_its_helpers_here() {
    let mut agent = Agent::in_a_pane();
    let mut logging = following(&agent.daemon);
    let quiet_helper = proto::pane_request::Create {
        quiet: true,
        ..helper("p2", "p9far", among_helpers("t2", "p9far's helpers"))
    };
    make(&mut agent.control, quiet_helper);
    let mut window = agent.daemon.connect();
    expect(&mut window, attending_request(), proto::Outcome::Done);
    let closed = |maker: &str| {
        pane(proto::pane_request::Request::MakerClosed(proto::pane_request::MakerClosed {
            maker: maker.to_string(),
            called: Some("🤖 far".to_string()),
        }))
    };

    expect(&mut agent.control, closed("p9far"), proto::Outcome::Done);
    logging.logged_until(r#""event":"msg.helper.orphans_told""#, Duration::from_secs(20));
    let helper = snapshot(&mut agent.control).panes.into_iter().find(|pane| pane.pane == "p2");
    assert!(helper.is_some_and(|helper| !helper.quiet && helper.maker_closed));
    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(read[0].contains("🤖 far closed; its helper "), "{read:?}");

    expect(&mut agent.control, closed("p9far"), proto::Outcome::AlreadySo);
    let refused = expect(&mut agent.control, closed("p1"), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("has not closed"), "{}", refused.answer.reason);
}
