//! Whether a window's `pane send` refuses text into an agent's prompt that already holds a draft.
//!
//! The next Return would submit the draft and the new text as one prompt, so the send is refused
//! naming the draft, and `--append` is how to add to it. The words are `muster_core::pane_text`'s
//! on both paths; this is the window's path end to end, against the fake agent with a prompt the
//! daemon reads, as `muster-cli`'s windowless suite runs the other.

use muster::proto::{
    OpenWindow, ReadPane, ReadWindow, Request, Response, SendToPane, Startup, request, response,
};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until, until_some};
use prost::Message;

#[test]
fn a_send_into_a_prompt_holding_a_draft_is_refused_and_an_append_is_not() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    accepted(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    }));
    accepted(request::Payload::OpenWindow(OpenWindow::default()));
    let pane = the_only_pane();

    accepted(send(&pane, "half typed", false, false));
    until(
        "the agent's prompt to show the draft",
        || read(&pane).contains("PROBE-PROMPT> half typed"),
        || format!("the pane shows {:?}", read(&pane)),
    );
    let refused = refused(answer(send(&pane, "/clear", true, false)))
        .expect("a send into a prompt holding a draft is refused");
    assert!(refused.contains("already holds \"half typed\""), "names the draft: {refused}");
    assert!(refused.contains("--append"), "says how to add to it: {refused}");

    accepted(send(&pane, " and more", false, true));
    accepted(send(&pane, " ", true, false));
    let heard = daemon.root().join("home/fake-agent-heard");
    until_some("the agent to read the draft it was sent with", || {
        let heard = std::fs::read_to_string(&heard).ok()?;
        heard.lines().any(|line| line == "half typed and more ").then_some(())
    });
    let heard = std::fs::read_to_string(&heard).unwrap();
    assert!(!heard.contains("/clear"), "a refused send was typed: {heard}");
}

fn send(pane: &str, text: &str, enter: bool, append: bool) -> request::Payload {
    request::Payload::SendToPane(SendToPane {
        pane_id: pane.to_string(),
        text: text.to_string(),
        enter,
        append,
        ..SendToPane::default()
    })
}

/// The window's only pane, by Muster's name for it.
fn the_only_pane() -> String {
    let mut found = None;
    until(
        "the window to hold the daemon's pane",
        || {
            let Some(response::Payload::Window(window)) =
                answer(request::Payload::ReadWindow(ReadWindow::default())).payload
            else {
                return false;
            };
            found = window.panes.first().map(|pane| pane.pane_id.clone());
            found.is_some()
        },
        || "the window never listed a pane, so there was nothing to send to".to_string(),
    );
    found.expect("the wait above returns only once there is one")
}

fn read(pane: &str) -> String {
    match answer(request::Payload::ReadPane(ReadPane {
        pane_id: pane.to_string(),
        ..ReadPane::default()
    }))
    .payload
    {
        Some(response::Payload::PaneText(text)) => text.text,
        _ => String::new(),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn accepted(payload: request::Payload) {
    if let Some(response::Payload::Failure(failure)) = answer(payload).payload {
        panic!("the core refused: {}", failure.reason);
    }
}

fn refused(response: Response) -> Option<String> {
    match response.payload {
        Some(response::Payload::Failure(failure)) => Some(failure.reason),
        _ => None,
    }
}
