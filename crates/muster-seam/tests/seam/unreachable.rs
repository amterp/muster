//! A pane whose daemon stops answering reads `unreachable`, against a real daemon.
//!
//! A devenv rebooted, and for two days the window listed its agents as `idle`, `done` and
//! blocked on the person, because each pane kept what its daemon last said and only the
//! machine's line said otherwise (kan a_2cJ6cgiLw). What the daemon last said is kept, beside
//! the state rather than as it, so that nothing reading a state takes the pane for a live one.

use muster::proto::{
    OpenWindow, ReadAsking, ReadWindow, Request, Response, Startup, request, response,
};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn a_pane_on_a_daemon_that_stops_answering_reads_unreachable_until_it_answers() {
    let _turn = muster::testing::fresh_session();
    // No bridge runs here, and a pane waiting for one would raise a problem of its own.
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let mut daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent_at_a_dialog("p1");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to see p1's agent blocked and asking",
        || pane("p1").is_some_and(|(state, _)| state == "blocked") && !read_asking().is_empty(),
        || format!("p1 reads {:?}, asking {:?}", pane("p1"), read_asking()),
    );

    daemon.kill();

    until(
        "p1 to read unreachable, keeping what it read before",
        || pane("p1").is_some_and(|(state, last)| state == "unreachable" && last == "blocked"),
        || format!("p1 reads {:?}", pane("p1")),
    );
    assert!(
        read_asking().is_empty(),
        "a pane on a daemon nobody can reach still asks for somebody: {:?}",
        read_asking()
    );

    daemon.restart();

    // The restart took its agent, so what it reads now is the new shell's; what matters is that
    // it is the daemon's word again.
    until(
        "p1 to read what its daemon says again",
        || pane("p1").is_some_and(|(state, last)| state != "unreachable" && last.is_empty()),
        || format!("p1 reads {:?}", pane("p1")),
    );
}

/// What the window says `pane` is doing, and what it read before its daemon stopped answering.
fn pane(pane: &str) -> Option<(String, String)> {
    read_window()
        .panes
        .into_iter()
        .find(|agent| agent.pane_id == pane)
        .map(|agent| (agent.state, agent.last_state))
}

trait Asked {
    fn is_empty(&self) -> bool;
}

impl Asked for muster::proto::Asking {
    fn is_empty(&self) -> bool {
        self.pane_id.is_empty() && self.group.is_empty()
    }
}

fn read_asking() -> muster::proto::Asking {
    match answer(request::Payload::ReadAsking(ReadAsking {})).payload {
        Some(response::Payload::Asking(asking)) => asking,
        other => panic!("the core answered a ReadAsking with {other:?}"),
    }
}

fn read_window() -> muster::proto::Window {
    match answer(request::Payload::ReadWindow(ReadWindow::default())).payload {
        Some(response::Payload::Window(window)) => window,
        other => panic!("asking what the window is showing answered {other:?}"),
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(
            response::Payload::Ok(_) | response::Payload::Made(_) | response::Payload::Opened(_),
        ) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
