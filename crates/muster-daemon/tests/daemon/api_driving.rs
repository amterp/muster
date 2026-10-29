//! Driving a pane through the API (MIP-10, section 5): typing into it with the read-back
//! `muster pane send --confirm` makes, closing it, and compacting and restarting its agent.

use crate::restart::reported;
use crate::support::*;
use api::event::Event;
use api::pane_request::Request as Pane;
use muster_harness::Input;

fn send(pane: &str, text: &str, confirm: bool) -> api::request::Request {
    on_pane(Pane::Send(api::SendToPane {
        pane: pane.to_string(),
        text: text.to_string(),
        enter: true,
        confirm,
        ..api::SendToPane::default()
    }))
}

fn close(pane: &str) -> api::request::Request {
    on_pane(Pane::Close(api::ClosePane { pane: pane.to_string() }))
}

fn compact(pane: &str, focus: &str) -> api::request::Request {
    on_pane(Pane::Compact(api::CompactAgent {
        pane: pane.to_string(),
        focus: focus.to_string(),
        ..api::CompactAgent::default()
    }))
}

fn restart(pane: &str) -> api::request::Request {
    on_pane(Pane::Restart(api::RestartAgent { pane: pane.to_string(), ..Default::default() }))
}

fn refused_as(asked: &ApiAsked, refusal: api::Refusal) {
    assert_eq!((asked.outcome(), asked.refusal()), (api::Outcome::Refused, refusal), "{asked:?}");
}

/// The card's proof: a program makes a shell pane, types into it and is told the text arrived,
/// reads what it printed, and closes it, hearing the close before the answer and knowing it as
/// its own.
#[test]
fn a_client_makes_a_shell_types_into_it_reads_the_confirmation_and_closes_it() {
    let daemon = daemon();
    let mut api = daemon.api();
    api.subscribe(&[api::Topic::Panes]);
    let made = api
        .ask(new_pane(api::NewPane { operation: "op-1".to_string(), ..api::NewPane::default() }));
    let Some(api::answer::Detail::Made(made)) = made.answer.detail else {
        panic!("pane.new answered {:?}", made.answer)
    };
    let mut control = daemon.connect();
    until_text(&mut control, &made.pane, "$");

    let typed = api.ask(send(&made.pane, "echo driven-$((6*7))", true));
    assert_eq!(typed.outcome(), api::Outcome::Done, "{:?}", typed.answer);
    until_some("the pane to print what it was sent", || {
        let read = api.ask(read_pane(&made.pane, None));
        read.pane_text().text.lines().any(|row| row.trim_end() == "driven-42").then_some(())
    });

    let closed = api.ask(close(&made.pane));
    assert_eq!(closed.outcome(), api::Outcome::Done, "{:?}", closed.answer);
    let heard = closed.events.iter().find(
        |event| matches!(&event.event, Some(Event::PaneClosed(gone)) if gone.pane == made.pane),
    );
    let heard = heard.unwrap_or_else(|| panic!("no close before the answer: {:?}", closed.events));
    assert_eq!(heard.correlation, closed.answer.correlation, "the close is the request's own");

    refused_as(&api.ask(close(&made.pane)), api::Refusal::NotFound);
}

/// A close made on one connection is heard on another as nobody's request of its own.
#[test]
fn another_connection_hears_a_close_with_no_correlation() {
    let daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    let mut watching = daemon.api();
    watching.subscribe(&[api::Topic::Panes]);

    let mut acting = daemon.api();
    assert_eq!(acting.ask(close("p1")).outcome(), api::Outcome::Done);

    let heard = watching.event_matching(
        "p1 to close",
        |event| matches!(&event.event, Some(Event::PaneClosed(gone)) if gone.pane == "p1"),
    );
    assert_eq!(heard.correlation, "");
}

#[test]
fn a_send_that_cannot_be_typed_is_refused_with_a_code_and_types_nothing() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let mut api = daemon.api();

    let nothing = on_pane(Pane::Send(api::SendToPane {
        pane: "p1".to_string(),
        ..api::SendToPane::default()
    }));
    refused_as(&api.ask(nothing), api::Refusal::Invalid);
    let unread_key = on_pane(Pane::Send(api::SendToPane {
        pane: "p1".to_string(),
        keys: vec!["down".to_string(), "no-such-key".to_string()],
        ..api::SendToPane::default()
    }));
    refused_as(&api.ask(unread_key), api::Refusal::Invalid);
    refused_as(&api.ask(send("p9", "hello", false)), api::Refusal::NotFound);

    // Somebody's draft in the agent's prompt, which a Return would send with whatever was added.
    Input::connect(daemon.socket_path()).send(
        "p1",
        proto::input_event::Input::Send(proto::input_event::Send {
            text: "half typed".to_string(),
            ..Default::default()
        }),
    );
    until_some("the prompt to hold the draft", || prompt_holds(&mut control));
    let into_draft = api.ask(send("p1", "do the thing", true));
    refused_as(&into_draft, api::Refusal::Unspecified);
    assert!(into_draft.answer.reason.contains("half typed"), "{:?}", into_draft.answer);
    assert_eq!(prompt_holds(&mut control).as_deref(), Some("half typed"), "nothing was added");
}

fn prompt_holds(control: &mut Control) -> Option<String> {
    let read = pane(proto::pane_request::Request::Read(proto::pane_request::Read {
        pane: "p1".to_string(),
        last: 1,
        prompt: true,
        ..Default::default()
    }));
    match expect(control, read, proto::Outcome::Done).answer.detail {
        Some(proto::answer::Detail::Text(text)) => text.prompt,
        other => panic!("a read answered with {other:?}"),
    }
}

#[test]
fn a_compaction_asked_through_the_api_is_typed_once_the_agent_is_idle() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", in_new_tab("t2")));
    daemon.run_agent("p1");
    daemon.set_agent_state("p1", proto::AgentState::Working);
    let mut api = daemon.api();

    refused_as(&api.ask(compact("p1", "keep\nthe notes")), api::Refusal::Invalid);
    refused_as(&api.ask(compact("p2", "")), api::Refusal::Unspecified);
    refused_as(&api.ask(compact("p9", "")), api::Refusal::NotFound);
    let asked = api.ask(compact("p1", "keep the parser notes"));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);

    daemon.set_agent_state("p1", proto::AgentState::Idle);
    let heard = daemon.root().join("home/fake-agent-heard");
    until_some("the agent to read its compaction", || {
        let heard = std::fs::read_to_string(&heard).unwrap_or_default();
        heard.lines().any(|line| line == "/compact keep the parser notes").then_some(())
    });
}

#[test]
fn a_restart_through_the_api_resumes_the_agent_and_says_what_it_typed() {
    let (daemon, _control) = reported(&[], "--model opus", "s-1");
    let mut api = daemon.api();

    let asked = api.ask(restart("p1"));
    assert_eq!(asked.outcome(), api::Outcome::Done, "{:?}", asked.answer);
    let restarted = asked.restarted();
    assert!(restarted.typed.ends_with("--model' 'opus' '--resume' 's-1'"), "{restarted:?}");
    assert!(!restarted.killed, "{restarted:?}");
    daemon.until_agent("p1", proto::AgentState::Idle);

    let reserved = on_pane(Pane::Restart(api::RestartAgent {
        pane: "p1".to_string(),
        env: [("MUSTER_PANE".to_string(), "p2".to_string())].into(),
    }));
    refused_as(&api.ask(reserved), api::Refusal::Invalid);
    refused_as(&api.ask(restart("p9")), api::Refusal::NotFound);
}

#[test]
fn an_agent_with_no_session_to_resume_is_left_running() {
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let mut api = daemon.api();

    let asked = api.ask(restart("p1"));
    refused_as(&asked, api::Refusal::Unspecified);
    let p1 = snapshot(&mut control).panes.into_iter().find(|record| record.pane == "p1");
    assert_eq!(p1.and_then(|record| record.agent).as_deref(), Some("claude"), "{asked:?}");
}
