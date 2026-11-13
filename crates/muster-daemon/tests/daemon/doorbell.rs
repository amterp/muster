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
    fn with_codex_session_commands() -> Agent {
        use std::os::unix::fs::PermissionsExt;

        let mut agent = Agent::in_a_pane();
        let home = agent.daemon.root().join("home");
        let path = home.join(".muster/agent-detection/claude.toml");
        let fake = std::fs::read_to_string(&path).unwrap();
        let codex = include_str!("../../../muster-detect/manifests/codex.toml");
        let commands = codex.split_once("[session]").unwrap().1.split_once("[[rules]]").unwrap().0;
        let (prefix, rest) = fake.split_once("[session]").unwrap();
        let rules = rest.split_once("[[rules]]").unwrap().1;
        // Codex's session table can use keys a newer engine reads, so the fake asks for that
        // engine too; otherwise the daemon refuses the whole manifest.
        let engine = |manifest: &str| {
            manifest.lines().find(|line| line.starts_with("min_engine_version")).unwrap().to_owned()
        };
        let prefix = prefix.replacen(&engine(&fake), &engine(codex), 1);
        // Keep the fake terminal, but exercise the session commands that actually ship for Codex.
        std::fs::write(&path, format!("{prefix}[session]{commands}[[rules]]{rules}")).unwrap();
        let binary = home.join(".muster/bin/codex");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/fake-codex-queued\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            home.join(".profile"),
            "PATH=\"$HOME/.muster/bin:$PATH\"\nexport PATH\nPS1='$ '\n",
        )
        .unwrap();
        let reload = session(proto::session_request::Request::SendManifests(
            proto::SendManifests::default(),
        ));
        let answer = agent.control.ask(reload);
        assert!(matches!(answer.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo));
        agent
    }

    fn codex_queued(&self) -> String {
        std::fs::read_to_string(self.daemon.root().join("home/fake-codex-queued"))
            .unwrap_or_default()
    }

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

#[test]
fn a_codex_post_uses_the_visible_prompt_with_an_old_session_reported() {
    let mut agent = Agent::with_codex_session_commands();
    agent.reports_session("old-conversation");
    agent.post("p1", "the next assignment");

    until_some("a typed wake or a queued copy", || {
        (!agent.rung().is_empty() || !agent.codex_queued().is_empty()).then_some(())
    });
    assert!(agent.codex_queued().is_empty(), "queued to {}", agent.codex_queued());
    assert_eq!(agent.rung().len(), 1);
    assert_eq!(agent.read_in_its_pane(), ["integrator: the next assignment"]);
}

#[test]
fn a_codex_read_while_a_draft_holds_delivery_leaves_no_stored_wake() {
    let mut agent = Agent::with_codex_session_commands();
    agent.reports_session("resumed-conversation");
    agent.type_in("half typed", false);
    agent.until_shows("PROBE-PROMPT> half typed");
    agent.post("p1", "read before resuming");

    until_some("delivery to reach the draft or the queue", || {
        (agent.unread().iter().any(|why| why.contains("typed and not sent"))
            || !agent.codex_queued().is_empty())
        .then_some(())
    });
    assert_eq!(agent.read_in_its_pane(), ["integrator: read before resuming"]);
    assert!(agent.codex_queued().is_empty(), "stored after read: {}", agent.codex_queued());
    std::thread::sleep(QUIET + Duration::from_secs(1));
    assert!(agent.rung().is_empty(), "rung for a message already read");
    assert_eq!(agent.prompt_holds().as_deref(), Some("half typed"));
}

#[test]
fn a_codex_urgent_post_still_reaches_its_running_turn() {
    let mut agent = Agent::with_codex_session_commands();
    agent.reports_session("running-conversation");
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    let posted = agent.post_urgently("p1", "change course");
    assert_eq!(posted.reached[0].activity(), msg_answer::Activity::Working);

    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
    assert!(agent.codex_queued().is_empty());
    let record = snapshot(&mut agent.control).panes.into_iter().find(|pane| pane.pane == "p1");
    assert_eq!(record.unwrap().agent_state(), proto::AgentState::Working);
    assert_eq!(agent.read_in_its_pane(), ["integrator: change course"]);
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
        agent
            .unread()
            .iter()
            .any(|why| why.contains("neither read it nor gone to work"))
            .then_some(())
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

/// An urgent ring at work whose Returns the agent swallows for longer than the doorbell's first
/// presses last is pressed again later in the same turn, once Returns are taken again: it was
/// typed for the turn it went into, and left for the turn's end it would sit there for all of it.
#[test]
fn an_urgent_ring_whose_returns_were_swallowed_is_sent_later_in_the_same_turn() {
    let mut agent = Agent::in_a_pane();
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.type_in("heedless 45", true);
    until_some("the agent to start discarding Returns", || {
        agent.heard().iter().any(|line| line == "heedless 45").then_some(())
    });
    std::thread::sleep(QUIET);
    let mut logging = following(&agent.daemon);

    agent.post_urgently("p1", "now");
    agent.until_shows(&format!("PROBE-PROMPT> [muster] {0}", agent.pair("p1")));
    // Its first Return and six more, five seconds apart, all swallowed.
    let left = r#""event":"msg.ring.left""#;
    logging.logged_until(left, ANSWER * 9);
    assert_eq!(agent.rung(), Vec::<String>::new(), "a Return reached an agent discarding them");

    let sent = r#""event":"msg.ring.left_sent""#;
    let logged = logging.logged_until(sent, ANSWER * 4);
    assert!(
        logged
            .iter()
            .any(|line| line.line.contains(sent) && line.line.contains(r#""at_work":"true""#)),
        "{logged:?}"
    );
    let unsent = |agent: &mut Agent| agent.unread().iter().any(|why| why.contains("not been sent"));
    assert!(unsent(&mut agent), "{:?}", agent.unread());
    let rung = agent.until_rung(1);
    assert!(rung[0].contains("1 urgent"), "{rung:?}");
    agent.daemon.until_agent("p1", proto::AgentState::Working);
    // Looked at again after the press that sent it, and forgotten: not said to sit unsent.
    until_some("the sent ring to be forgotten", || (!unsent(&mut agent)).then_some(()));
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

/// A ring whose Return the agent never takes, as a harness that swallows one does, is given up
/// with its text still in the prompt. Once the agent has read what it was rung for, the
/// ring is taken back, rather than left for the next Return anybody presses to send.
#[test]
fn a_ring_given_up_in_a_prompt_is_taken_back_once_its_messages_are_read() {
    let mut agent = heedless_agent();
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");
    let ended = r#""event":"msg.ring.ended""#;
    // Six Returns, five seconds apart, all swallowed.
    let logged = logging.logged_until(ended, ANSWER * 10);
    assert!(logged.iter().any(|line| line.line.contains(ended)), "{logged:?}");

    assert_eq!(agent.read_in_its_pane(), ["integrator: a brief"]);
    taken_back_before_the_next_line(&mut agent, &mut logging);
}

/// The same, read while the doorbell is still pressing Return for the ring.
#[test]
fn a_ring_whose_return_was_swallowed_is_taken_back_once_its_messages_are_read() {
    let mut agent = heedless_agent();
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");

    assert_eq!(agent.read_in_its_pane(), ["integrator: a brief"]);
    taken_back_before_the_next_line(&mut agent, &mut logging);
}

/// The fake agent idle in p1, discarding every Return typed into it from now on.
/// A ring returned and then a turn that did not take it, as Claude Code started one from a hook
/// with the ring still in its prompt: once the turn ends, Return is pressed for the ring again,
/// without its text typed again.
#[test]
fn a_ring_left_in_the_prompt_by_a_turn_that_did_not_take_it_is_sent_once_the_agent_is_idle() {
    let mut agent = heedless_agent();
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");
    agent.daemon.unblock_agent_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Working);
    std::thread::sleep(ANSWER + Duration::from_secs(1));
    assert_eq!(agent.rung(), Vec::<String>::new(), "a Return reached an agent discarding them");

    agent.daemon.end_turn_unasked();
    let sent = r#""event":"msg.ring.left_sent""#;
    let logged = logging.logged_until(sent, QUIET + ANSWER * 2);
    assert!(logged.iter().any(|line| line.line.contains(sent)), "{logged:?}");
    let rung = agent.until_rung(1);
    assert_eq!(rung.len(), 1, "typed again: {rung:?}");
}

/// The same, with something typed after the ring: a Return would send that too, so none is
/// pressed, and what sits in the prompt is left for whoever typed it.
#[test]
fn a_ring_left_in_the_prompt_is_not_sent_once_something_is_typed_after_it() {
    let mut agent = heedless_agent();
    agent.post("p1", "a brief");
    agent.until_shows("PROBE-PROMPT> [muster]");
    agent.daemon.unblock_agent_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Working);
    agent.type_in("z", false);
    std::thread::sleep(ANSWER + Duration::from_secs(1));

    agent.daemon.end_turn_unasked();
    agent.daemon.until_agent("p1", proto::AgentState::Idle);
    std::thread::sleep(QUIET + ANSWER * 2);
    assert_eq!(agent.heard(), ["heedless"], "a Return was pressed beside what was typed");
}

/// A ring whose every Return was swallowed is left once they run out, with no more pressed for it.
/// Once the harness takes Returns again, the next post for the pane finds the old ring alone in
/// the prompt and has Return pressed for it, rather than waiting on it as somebody's draft for
/// good; then the new wake is rung.
#[test]
fn a_ring_whose_returns_ran_out_is_sent_when_a_later_post_finds_it() {
    let mut agent = heedless_agent();
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");
    let unsent = r#""event":"msg.ring.left_unsent""#;
    // The ring's six Returns, five seconds apart, all swallowed, and no more after them.
    let logged = logging.logged_until(unsent, ANSWER * 10);
    assert!(logged.iter().any(|line| line.line.contains(unsent)), "{logged:?}");

    agent.daemon.end_turn_unasked();
    std::thread::sleep(QUIET + ANSWER);
    assert_eq!(agent.rung(), Vec::<String>::new(), "sent with its Returns spent");

    agent.post("p1", "and another");
    let rung = agent.until_rung(2);
    assert!(rung[0].contains("1 new"), "the old ring first: {rung:?}");
}

/// A post re-arms a spent left ring once: while its wake waits behind a prompt that keeps
/// swallowing Returns, the doorbell looks at the wake again every few seconds, and none of those
/// looks gives the ring more Returns.
#[test]
fn a_spent_left_ring_gets_its_returns_again_once_per_post() {
    let mut agent = heedless_agent();
    let mut logging = following(&agent.daemon);
    agent.post("p1", "a brief");
    let unsent = r#""event":"msg.ring.left_unsent""#;
    let logged = logging.logged_until(unsent, ANSWER * 10);
    assert!(logged.iter().any(|line| line.line.contains(unsent)), "{logged:?}");

    agent.post("p1", "and another");
    let logged = logging.logged_times_until(unsent, 2, ANSWER * 10);
    assert_eq!(logged.iter().filter(|line| line.line.contains(unsent)).count(), 2, "{logged:?}");
    let sent = r#""event":"msg.ring.left_sent""#;
    let before = logged.iter().filter(|line| line.line.contains(sent)).count();
    let logged = logging.logged_times_until(sent, before + 1, ANSWER * 4);
    let after = logged.iter().filter(|line| line.line.contains(sent)).count();
    assert_eq!(after, before, "Returns pressed again with no new post");
}

fn heedless_agent() -> Agent {
    let agent = Agent::in_a_pane();
    agent.type_in("heedless", true);
    until_some("the agent to start discarding Returns", || {
        agent.heard().iter().any(|line| line == "heedless").then_some(())
    });
    agent
}

/// Waits for the ring to be taken back, then sends a line of its own: what the agent reads says
/// whether the ring was still in front of it.
fn taken_back_before_the_next_line(agent: &mut Agent, logging: &mut Control) {
    let taken_back = r#""event":"msg.ring.taken_back""#;
    let logged = logging.logged_until(taken_back, Duration::from_secs(20));
    assert!(logged.iter().any(|line| line.line.contains(taken_back)), "{logged:?}");
    // A newline, which the agent reads though it discards Returns.
    agent.type_in("next\n", false);
    until_some("the agent to read the next line", || {
        agent.heard().iter().any(|line| line.ends_with("next")).then_some(())
    });
    assert_eq!(agent.heard(), ["heedless", "next"], "the ring was sent, or left in the prompt");
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

/// A session whose agent reported its id is woken by the command its manifest names: nothing is
/// typed into the pane, so a draft there neither holds the wake back
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
/// one typed: a prompt hook can say no wait for a command-started turn too, with nothing typed.
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

/// A command must not bypass a blocked prompt: the harness may accept the wake without starting
/// a turn for it while a dialog is open.
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

/// A command can accept a wake without starting a turn. If its agent neither works nor reads,
/// the wake is typed instead and the command is not tried again for that session.
#[test]
fn a_command_wake_nobody_takes_is_typed_instead() {
    let mut agent = Agent::in_a_pane();
    agent.reports_session("dead");
    std::thread::sleep(QUIET);
    agent.post("p1", "a brief");
    until_some("the wake command to run", || (!agent.woken().is_empty()).then_some(()));
    muster_harness::until_within(
        "the wake to be typed instead",
        COMMAND_ANSWER + muster_harness::PATIENCE,
        || !agent.rung().is_empty(),
        || format!("heard: {:?}", agent.heard()),
    );
    assert_eq!(agent.rung().len(), 1);
    assert_eq!(agent.woken().len(), 1, "the command was run again");
}

fn declare_review_wait(agent: &mut Agent) {
    make(&mut agent.control, create("p2", in_new_tab("t2")));
    let report = proto::pane_request::Report {
        pane: "p1".to_string(),
        waiting: Some("the review".to_string()),
        waiting_until_message_from: Some("p2".to_string()),
        ..Default::default()
    };
    expect(
        &mut agent.control,
        pane(proto::pane_request::Request::Report(report)),
        proto::Outcome::Done,
    );
}

fn review_wait(agent: &mut Agent) -> Option<String> {
    snapshot(&mut agent.control)
        .panes
        .into_iter()
        .find(|p| p.pane == "p1")
        .and_then(|p| p.facts?.waiting)
}

#[test]
fn a_sender_wait_outlasts_an_urgent_ring_waiting_for_its_return() {
    let mut agent = Agent::in_a_pane();
    declare_review_wait(&mut agent);
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.posting_as("p2", "p1", "the review", true);
    agent.until_shows("PROBE-PROMPT> [muster]");
    assert_eq!(review_wait(&mut agent).as_deref(), Some("the review"), "the ring is still unsent");
    agent.daemon.block_agent_unasked();
    agent.until_shows("PROBE-STATE:BLOCKED");
    assert_eq!(
        review_wait(&mut agent).as_deref(),
        Some("the review"),
        "the dialog holds its Return"
    );
}

#[test]
fn a_sender_wait_outlasts_a_command_its_agent_has_not_taken() {
    let mut agent = Agent::in_a_pane();
    declare_review_wait(&mut agent);
    agent.reports_session("dead");
    let mut logging = following(&agent.daemon);
    agent.posting_as("p2", "p1", "the review", false);
    logging.logged_until(r#""event":"msg.rang""#, Duration::from_secs(20));
    assert_eq!(
        review_wait(&mut agent).as_deref(),
        Some("the review"),
        "the command only stored it"
    );
    agent.read_in_its_pane();
    assert_eq!(review_wait(&mut agent), None, "reading delivers the review");
}

/// A command-delivered wake whose turn starts late is not also typed, which would start a
/// duplicate turn. The timing came from Codex's former queue route
/// (docs/observations/codex-0.163.0.md).
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

/// A pane at a dialog that a message has waited on for half an hour is told to the person, once:
/// the poster heard at two minutes, and is often an idle agent that cannot answer the dialog.
#[test]
fn a_pane_at_a_dialog_a_message_waited_on_for_half_an_hour_is_told_to_the_person() {
    // Half an hour is fifteen stalls, so fifteen seconds here.
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "1000")]);
    let mut logging = following(&agent.daemon);
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    agent.post("p1", "a brief");

    let escalated = r#""event":"msg.group.escalated""#;
    let logged = logging.logged_until(escalated, Duration::from_secs(40));
    assert!(logged.iter().any(|line| line.line.contains(escalated)), "never told the person");
    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].contains("@muster: ")
            && read[0].contains(" (p1) has had #4, from integrator,")
            && read[0].contains("it is at a dialog")
            && read[0].contains("`muster pane show --pane "),
        "{read:?}"
    );
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told twice");
}

/// A stall whose poster is at work is not told while it works, since it would read the notice
/// only once its turn ends; and once the member has read the message, nothing is told at all.
#[test]
fn a_stall_that_ends_while_its_poster_works_is_never_told() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "12000")]);
    make(&mut agent.control, create("p2", beside("p1", proto::Side::Right)));
    agent.daemon.run_agent("p2");
    let mut logging = following(&agent.daemon);
    agent.type_in("deaf 40", true);
    std::thread::sleep(Duration::from_secs(2));
    let poster = msg_request::Caller { pane: Some("p2".to_string()), ..Default::default() };
    let brief = Asked::Post(msg_request::Post {
        body: "a brief".to_string(),
        to: vec!["p1".to_string()],
        ..Default::default()
    });
    let brief =
        Service::Msg(proto::MsgRequest { caller: Some(poster.clone()), request: Some(brief) });
    expect(&mut agent.control, brief, proto::Outcome::Done);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);

    let stalled = r#""event":"msg.group.stalled""#;
    let logged = logging.logged_until(stalled, Duration::from_secs(40));
    assert!(
        logged
            .iter()
            .any(|line| line.line.contains(stalled) && line.line.contains(r#""unlock":"tell""#)),
        "{logged:?}"
    );
    let told = r#""event":"msg.group.told""#;
    let logged = logging.logged_until(told, Duration::from_secs(6));
    assert!(!logged.iter().any(|line| line.line.contains(told)), "told at work");

    assert_eq!(agent.read_in_its_pane().len(), 1, "the brief");
    let settled = r#""event":"msg.group.untold_settled""#;
    let logged = logging.logged_until(settled, Duration::from_secs(20));
    let line = logged.iter().find(|line| line.line.contains(settled));
    assert!(line.is_some_and(|line| line.line.contains(r#""to":"p2""#)), "{logged:?}");
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(agent.reading(poster), Vec::<String>::new(), "told of a stall that had ended");
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
    // What the doorbell judged, which the debug lines before it held only while they lasted: the
    // rule that read the screen, its shape and its title's, and how long it had been still.
    for field in ["rule", "shape", "drawn_ms_ago"] {
        let named = format!(r#""{field}":"#);
        assert!(line.line.contains(&named), "{field} missing from {}", line.line);
        let empty = format!(r#"{named}"""#);
        assert!(!line.line.contains(&empty), "{field} empty in {}", line.line);
    }
    assert!(line.line.contains(r#""title":"#), "title missing from {}", line.line);

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

/// A helper whose declared wait ran out with nothing ending it went idle for a known reason, and
/// its maker is told that rather than a guess that its turn ended on an error.
#[test]
fn a_helper_whose_wait_ran_out_tells_its_maker_so() {
    let mut agent = Agent::in_a_pane_with(&[
        ("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000"),
        ("MUSTER_DAEMON_DECLARED_WAIT_MS", "3000"),
    ]);
    // Nothing wakes the agent when its jobs end, as for Codex, so its declared wait is limited.
    let path = agent.daemon.root().join("home/.muster/agent-detection/claude.toml");
    let manifest = std::fs::read_to_string(&path).expect("the harness wrote it");
    let unwoken: String = manifest
        .lines()
        .filter(|line| !line.starts_with("wakes_when_jobs_end"))
        .flat_map(|line| [line, "\n"])
        .collect();
    assert_ne!(unwoken, manifest, "the harness's manifest says its jobs wake it");
    std::fs::write(&path, unwoken).expect("the override is rewritten");
    let send =
        session(proto::session_request::Request::SendManifests(proto::SendManifests::default()));
    let asked = agent.control.ask(send);
    assert!(matches!(asked.outcome(), proto::Outcome::Done | proto::Outcome::AlreadySo));

    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    let declare = proto::pane_request::Report {
        pane: "p2".to_string(),
        waiting: Some("the full gate".to_string()),
        ..Default::default()
    };
    expect(
        &mut agent.control,
        pane(proto::pane_request::Request::Report(declare)),
        proto::Outcome::Done,
    );
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);

    logging.logged_until(r#""event":"daemon.wait.over""#, Duration::from_secs(20));
    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    let read = agent.read_in_its_pane();
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].contains(
            "the wait it declared on the full gate ran out after 3s with nothing ending it."
        ),
        "{read:?}"
    );
}

#[test]
fn a_helpers_unlimited_wait_is_noticed_without_ending_it() {
    long_helper_wait(None);
}

#[test]
fn a_helpers_live_process_wait_is_noticed_without_ending_it() {
    struct Job(std::process::Child);
    impl Drop for Job {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let job = Job(std::process::Command::new("sleep").arg("600").spawn().unwrap());
    long_helper_wait(Some(job.0.id()));
}

fn long_helper_wait(pid: Option<u32>) {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_WAIT_MS", "2000")]);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    let declare = || {
        pane(proto::pane_request::Request::Report(proto::pane_request::Report {
            pane: "p2".to_string(),
            waiting: Some("the full gate".to_string()),
            waiting_until_exits: pid,
            ..Default::default()
        }))
    };
    let mut logging = following(&agent.daemon);
    expect(&mut agent.control, declare(), proto::Outcome::Done);
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    let read = agent.read_in_its_pane();
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].contains("had been waiting on the full gate for ")
            && read[0].contains(" at ")
            && read[0].contains("Z. "),
        "{read:?}"
    );
    let held = |control: &mut Control| {
        snapshot(control).panes.into_iter().find(|p| p.pane == "p2").unwrap().facts.unwrap().waiting
    };
    assert_eq!(held(&mut agent.control).as_deref(), Some("the full gate"));

    let mut logging = following(&agent.daemon);
    expect(&mut agent.control, declare(), proto::Outcome::Done);
    logging.logged_times_until(r#""event":"msg.helper.told""#, 2, Duration::from_secs(20));
    assert_eq!(agent.read_in_its_pane().len(), 1, "a replacement wait is noticed again");

    let replaced = agent.daemon.replace(None);
    assert_eq!(replaced.outcome(), proto::Outcome::Done, "{}", replaced.reason);
    agent.control = agent.daemon.connect();
    // Prove no second notice past the test's two-second notice limit after takeover.
    std::thread::sleep(Duration::from_secs(3));
    assert!(agent.read_as("p1").is_empty());
    assert_eq!(held(&mut agent.control).as_deref(), Some("the full gate"));
}

/// A helper waiting on a message from a pane that then closes has no wait left to notice: its
/// maker is not told after the notice limit that it is still waiting.
#[test]
fn a_helper_waiting_on_a_pane_that_closes_is_not_noticed_waiting() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_WAIT_MS", "2000")]);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "helpers")));
    make(&mut agent.control, create("p3", in_new_tab("t3")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    expect(
        &mut agent.control,
        pane(proto::pane_request::Request::Report(proto::pane_request::Report {
            pane: "p2".to_string(),
            waiting: Some("the review".to_string()),
            waiting_until_message_from: Some("p3".to_string()),
            ..Default::default()
        })),
        proto::Outcome::Done,
    );
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    crate::reopen::close(&mut agent.control, "p3");

    std::thread::sleep(Duration::from_secs(5));
    assert!(agent.read_in_its_pane().is_empty(), "the maker was told of a wait that had ended");
    let held = snapshot(&mut agent.control)
        .panes
        .into_iter()
        .find(|p| p.pane == "p2")
        .and_then(|p| p.facts)
        .and_then(|facts| facts.waiting);
    assert_eq!(held, None);
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

/// A helper at a dialog its maker leaves unanswered is told to the person once half an hour has
/// passed, which is 180 settles: the maker heard at the settle, and did nothing.
#[test]
fn a_helpers_dialog_its_maker_leaves_is_told_to_the_person() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "100")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);
    logging.logged_until(r#""event":"msg.helper.told""#, Duration::from_secs(20));
    assert_eq!(agent.read_in_its_pane().len(), 1, "the maker hears first");

    let escalated = r#""event":"msg.helper.escalated""#;
    let logged = logging.logged_until(escalated, Duration::from_secs(40));
    assert!(logged.iter().any(|line| line.line.contains(escalated)), "never told the person");
    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].contains("@muster: ")
            && read[0].contains(" (p2), a helper of ")
            && read[0].contains("its maker has not answered it")
            && read[0].contains("`muster pane show --pane "),
        "{read:?}"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told twice");
}

/// A member still holding its group up as a newer daemon takes its pane over was told of to its
/// poster already, and the new daemon does not tell it again: Muster says a stall once.
#[test]
fn a_stall_told_before_a_handoff_is_not_told_again() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "8000")]);
    agent.post("p1", "a brief");
    agent.until_rung(1);
    let mut logging = following(&agent.daemon);
    logging.logged_until(r#""event":"msg.group.told""#, Duration::from_secs(40));
    assert_eq!(agent.read_as("integrator").len(), 1, "told before the handoff");

    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    agent.control = agent.daemon.connect();
    // Two stall lengths on the new daemon: a wake again and then a notice, were it told afresh.
    std::thread::sleep(Duration::from_secs(20));
    assert_eq!(agent.read_as("integrator"), Vec::<String>::new(), "told again");
}

/// A helper's dialog the person was told of before a newer daemon took its pane over is not
/// told to them again, and the newer daemon goes on timing the same block rather than starting it
/// over.
#[test]
fn a_helpers_dialog_told_to_the_person_is_not_told_again_after_a_handoff() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "100")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Blocked);
    logging.logged_until(r#""event":"msg.helper.escalated""#, Duration::from_secs(40));
    assert_eq!(agent.reading(msg_request::Caller::default()).len(), 1, "told before");

    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    agent.control = agent.daemon.connect();
    until_some("the helper to read blocked on the new daemon", || {
        let panes = snapshot(&mut agent.control).panes;
        let helper = panes.into_iter().find(|pane| pane.pane == "p2")?;
        (helper.agent_state() == proto::AgentState::Blocked).then_some(())
    });
    // More than the 18 seconds a block takes to reach the person at this settle.
    std::thread::sleep(Duration::from_secs(24));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told again");
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

/// A post from the agent in `pane` to `to`, as `muster msg post --to` from inside that pane.
fn post_from_pane(control: &mut Control, pane: &str, to: &str, body: &str) {
    let asked = Asked::Post(msg_request::Post {
        body: body.to_string(),
        to: vec![to.to_string()],
        ..Default::default()
    });
    let caller = msg_request::Caller { pane: Some(pane.to_string()), ..Default::default() };
    let service = Service::Msg(proto::MsgRequest { caller: Some(caller), request: Some(asked) });
    expect(control, service, proto::Outcome::Done);
}

/// A helper that ended its turn with a post to somebody other than its maker has reported, and
/// one that went idle while a helper of its own works is waiting on that work: neither is told
/// to its maker as gone idle without posting.
#[test]
fn a_helper_that_posted_to_anybody_or_waits_on_its_own_helpers_is_not_reported() {
    let mut agent = a_maker_with_idle_helpers(&[("p2", "p1"), ("p3", "p2")]);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    post_from_pane(&mut agent.control, "p2", "p3", "a brief for my reviewer");
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "posted to its own helper");

    agent.type_in_pane("p2", "the next brief", true);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    agent.daemon.set_agent_state("p3", proto::AgentState::Working);
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.read_in_its_pane(), Vec::<String>::new(), "its own helper works");
}

/// A pane left at a dialog with nothing waiting on it is told to the person once it has sat
/// there long enough, two hours outside a test, and only once.
#[test]
fn a_pane_at_a_dialog_for_hours_is_told_to_the_person_once() {
    let mut agent = Agent::in_a_pane_with(&[
        ("MUSTER_DAEMON_LONG_BLOCK_MS", "1000"),
        ("MUSTER_DAEMON_STALL_MS", "2000"),
    ]);
    let mut logging = following(&agent.daemon);
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);

    let told = r#""event":"msg.pane.long_block_told""#;
    let logged = logging.logged_until(told, Duration::from_secs(30));
    assert!(logged.iter().any(|line| line.line.contains(told)), "never told the person");
    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].starts_with("@muster: ")
            && read[0].contains(" (p1) has been at a dialog for ")
            && read[0].contains("`muster pane show --pane "),
        "{read:?}"
    );
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told twice");
}

/// A quiet pane promises no notice when it blocks, and its long dialog is not told either.
#[test]
fn a_quiet_pane_at_a_dialog_for_hours_is_not_told() {
    let daemon = Daemon::start_detecting_with(&[
        ("MUSTER_DAEMON_LONG_BLOCK_MS", "1000"),
        ("MUSTER_DAEMON_STALL_MS", "2000"),
    ]);
    let mut control = daemon.connect();
    let quiet = proto::pane_request::Create { quiet: true, ..create("p1", in_new_tab("t1")) };
    make(&mut control, quiet);
    daemon.run_agent("p1");
    let mut agent = Agent { daemon, control };
    agent.daemon.set_agent_state("p1", proto::AgentState::Working);
    agent.daemon.set_agent_state("p1", proto::AgentState::Blocked);
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told");
}

/// A helper its maker closed with the maker's own message unread holds nobody up: the maker
/// closed it on purpose and is not told it never read it. One closed with somebody else's
/// message unread still is, since they did not close it.
#[test]
fn a_helper_closed_with_only_its_makers_message_unread_is_no_stall() {
    let mut agent = a_maker_with_idle_helpers_and(
        &[("MUSTER_DAEMON_STALL_MS", "2000")],
        &[("p2", "p1"), ("p3", "p1")],
    );
    let mut logging = following(&agent.daemon);
    post_from_pane(&mut agent.control, "p1", "p2", "Thanks, that is everything.");
    agent.posting_as("critic", "p3", "one more question", false);
    expect(&mut agent.control, close_request("p2"), proto::Outcome::Done);
    expect(&mut agent.control, close_request("p3"), proto::Outcome::Done);

    logging.logged_until(r#""event":"msg.group.told""#, Duration::from_secs(30));
    let critic = agent.read_as("critic");
    assert_eq!(critic.len(), 1, "{critic:?}");
    assert!(critic[0].contains("no agent is in its pane any more"), "{critic:?}");
    std::thread::sleep(Duration::from_secs(3));
    let maker = agent.read_in_its_pane();
    assert!(maker.iter().all(|said| !said.contains("(p2)")), "the maker was told: {maker:?}");
}

/// A pane closed by the agent whose message it had not read holds nobody up: that agent closed
/// it on purpose. One a person closed with the same message unread still tells the agent.
#[test]
fn a_pane_closed_by_the_agent_waiting_on_it_is_no_stall() {
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_STALL_MS", "2000")]);
    for (pane, tab) in [("p2", "t2"), ("p3", "t3")] {
        make(&mut agent.control, create(pane, in_new_tab(tab)));
        agent.daemon.run_agent(pane);
    }
    let mut logging = following(&agent.daemon);
    post_from_pane(&mut agent.control, "p1", "p2", "Thanks, that is all.");
    agent.read_in_its_pane();
    post_from_pane(&mut agent.control, "p1", "p3", "One more thing.");
    expect(&mut agent.control, close_request_by("p2", "p1"), proto::Outcome::Done);
    expect(&mut agent.control, close_request("p3"), proto::Outcome::Done);

    logging.logged_until(r#""event":"msg.group.told""#, Duration::from_secs(30));
    std::thread::sleep(Duration::from_secs(3));
    let told = agent.read_in_its_pane();
    assert!(told.iter().any(|said| said.contains("(p3)")), "a person's close: {told:?}");
    assert!(told.iter().all(|said| !said.contains("(p2)")), "its own close: {told:?}");
}

/// A daemon whose helper watch settles in a second, with the fake agent idle in pane `p1` and in
/// each of `helpers`, quiet helpers each in a hidden tab of its own: `(helper, maker)`.
fn a_maker_with_idle_helpers(helpers: &[(&str, &str)]) -> Agent {
    a_maker_with_idle_helpers_and(&[], helpers)
}

/// [`a_maker_with_idle_helpers`], on a daemon with more in its environment.
fn a_maker_with_idle_helpers_and(environment: &[(&str, &str)], helpers: &[(&str, &str)]) -> Agent {
    let mut said = vec![("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")];
    said.extend_from_slice(environment);
    let mut agent = Agent::in_a_pane_with(&said);
    for (index, (helper_pane, maker)) in helpers.iter().enumerate() {
        let tab = format!("t{}", index + 2);
        let quiet_helper = proto::pane_request::Create {
            quiet: true,
            ..helper(helper_pane, maker, among_helpers(&tab, "helpers"))
        };
        make(&mut agent.control, quiet_helper);
        agent.daemon.run_agent(helper_pane);
    }
    agent
}

fn open_panes(control: &mut Control) -> Vec<String> {
    snapshot(control).panes.into_iter().map(|pane| pane.pane).collect()
}

/// A helper idle as its maker closes is closed with it once the settle has passed, as a person's
/// close would close it, so a reopen can bring it back, and the person is told nothing.
#[test]
fn an_idle_helper_closes_with_its_maker_and_nobody_is_told() {
    let mut agent = a_maker_with_idle_helpers(&[("p2", "p1")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, create("p9", in_new_tab("t9")));

    expect(&mut agent.control, close_request("p1"), proto::Outcome::Done);
    logging.logged_until(r#""event":"daemon.helper.closed_with_maker""#, Duration::from_secs(20));
    logging.logged_until(r#""event":"msg.helper.orphans_closed""#, Duration::from_secs(20));

    let snapshot = snapshot(&mut agent.control);
    let open: Vec<&str> = snapshot.panes.iter().map(|pane| pane.pane.as_str()).collect();
    assert_eq!(open, ["p9"]);
    let remembered: Vec<&str> = snapshot.closed.iter().map(|pane| pane.pane.as_str()).collect();
    assert_eq!(remembered, ["p2", "p1"], "newest first, each for a reopen to bring back");
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new());
}

/// A helper at work as its maker closes runs on, no longer quiet, and the person is told once
/// that it closes when idle (MIP-7, section 6, as changed since). It closes once its agent has
/// been idle for the settle.
#[test]
fn a_helper_at_work_outlives_its_maker_until_it_is_idle() {
    let mut agent = a_maker_with_idle_helpers(&[("p2", "p1")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, create("p9", in_new_tab("t9")));
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
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
    assert!(helper.is_some_and(|helper| !helper.quiet && helper.maker_closed));
    let read = agent.reading(msg_request::Caller::default());
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(
        read[0].starts_with("@muster: ")
            && read[0].contains("is still running, and closes once it is idle"),
        "{read:?}"
    );

    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);
    logging.logged_until(r#""event":"daemon.helper.closed_with_maker""#, Duration::from_secs(20));
    assert_eq!(open_panes(&mut agent.control), ["p9"]);
    assert_eq!(agent.reading(msg_request::Caller::default()), Vec::<String>::new(), "told once");
}

/// A helper closing with its maker is its own helpers' maker closing, so they follow it.
#[test]
fn a_helpers_helper_closes_after_it() {
    let mut agent = a_maker_with_idle_helpers(&[("p2", "p1"), ("p3", "p2")]);
    let mut logging = following(&agent.daemon);
    make(&mut agent.control, create("p9", in_new_tab("t9")));

    expect(&mut agent.control, close_request("p1"), proto::Outcome::Done);
    logging.logged_times_until(
        r#""event":"daemon.helper.closed_with_maker""#,
        2,
        Duration::from_secs(30),
    );
    assert_eq!(open_panes(&mut agent.control), ["p9"]);
}

/// A helper marked before its daemon restarted closes once it is at rest after it, as one whose
/// maker closes now does: the mark is in the state file, and nothing else is needed.
#[test]
fn a_helper_marked_before_a_restart_closes_once_idle_after_it() {
    let mut agent = Agent::in_a_pane_with(&[
        ("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000"),
        ("MUSTER_DAEMON_AGENT_TO_COME_MS", "1000"),
    ]);
    make(&mut agent.control, helper("p2", "p1", among_helpers("t2", "p1's helpers")));
    agent.daemon.run_agent("p2");
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    make(&mut agent.control, create("p9", in_new_tab("t9")));
    expect(&mut agent.control, close_request("p1"), proto::Outcome::Done);
    until_saved(&agent.daemon, "maker_closed");

    stop(&mut agent.daemon, &mut agent.control);
    agent.daemon.restart();
    agent.control = agent.daemon.connect();

    until_some("the marked helper to close with its maker", || {
        let snapshot = snapshot(&mut agent.control);
        let closed = snapshot.closed.iter().find(|closed| closed.pane == "p2")?;
        closed.closed_with_maker.then_some(())
    });
    assert_eq!(open_panes(&mut agent.control), ["p9"]);
}

/// A helper somebody brought on screen after its maker closed is theirs to close: it stays open
/// when its agent goes idle.
#[test]
fn a_helper_brought_forward_after_its_maker_closed_is_kept() {
    let mut agent = a_maker_with_idle_helpers(&[("p2", "p1")]);
    let mut logging = following(&agent.daemon);
    agent.daemon.set_agent_state("p2", proto::AgentState::Working);
    expect(&mut agent.control, close_request("p1"), proto::Outcome::Done);

    let show = proto::tab_request::SetHidden { tab: "t2".to_string(), hidden: false, label: None };
    let show = tab(proto::tab_request::Request::SetHidden(show));
    expect(&mut agent.control, show, proto::Outcome::Done);
    logging.logged_until(r#""event":"daemon.helper.kept""#, Duration::from_secs(20));
    agent.daemon.set_agent_state("p2", proto::AgentState::Idle);

    // Three settles, in which a helper not kept would have closed.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(open_panes(&mut agent.control), ["p2"]);
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
    let mut agent = Agent::in_a_pane_with(&[("MUSTER_DAEMON_HELPER_SETTLE_MS", "1000")]);
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

    // An agent in it that goes idle closes it, as a maker closing here would.
    agent.daemon.run_agent("p2");
    logging.logged_until(r#""event":"daemon.helper.closed_with_maker""#, Duration::from_secs(30));
    assert_eq!(open_panes(&mut agent.control), ["p1"]);
}

#[test]
fn a_prompt_cover_is_rechecked_on_takeover_and_leaves_with_its_agent() {
    let mut agent = Agent::in_a_pane();
    agent.type_in("menu", true);
    agent.until_shows("PROBE-MENU");
    until(
        "the pane to describe its covering menu",
        || snapshot(&mut agent.control).panes[0].prompt_covered_by.is_some(),
        || "the menu has not been described".to_string(),
    );
    let answer = agent.daemon.replace(None);
    assert_eq!(answer.outcome(), proto::Outcome::Done, "{}", answer.reason);
    agent.control = agent.daemon.connect();
    until(
        "the successor to read the still visible menu",
        || snapshot(&mut agent.control).panes[0].prompt_covered_by.is_some(),
        || "the covering menu was not read after takeover".to_string(),
    );
    agent.type_in("quit", true);
    until(
        "the covering menu to leave with the agent",
        || {
            let panes = snapshot(&mut agent.control).panes;
            panes[0].prompt_covered_by.is_none()
        },
        || "the exited agent still has a covering menu".to_string(),
    );
}
