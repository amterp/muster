//! Whether marking an agent's pane unread from a window makes it read `done`, as its daemon then
//! holds it, and whether a pane with nothing to mark is refused with the daemon's reason.

use muster::proto::{
    MarkUnread, OpenWindow, ReadWindow, Request, Response, Startup, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_harness::requests::{beside, create, in_new_tab, make};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn an_idle_agent_marked_unread_reads_done_and_a_shell_is_refused() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_detecting();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", daemon_proto::Side::Right)));
    daemon.run_agent("p1");
    accepted(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    }));
    accepted(request::Payload::OpenWindow(OpenWindow::default()));
    until(
        "the window to paint p1's agent idle and markable",
        || state_of("p1").is_some_and(|(state, markable)| state == "idle" && markable),
        || format!("p1 reads {:?}", state_of("p1")),
    );

    accepted(mark("p1"));
    until(
        "p1 to read done",
        || state_of("p1").is_some_and(|(state, markable)| state == "done" && !markable),
        || format!("p1 reads {:?}", state_of("p1")),
    );
    let Some(response::Payload::Failure(failure)) = answer(mark("p2")).payload else {
        panic!("a shell was marked unread")
    };
    assert!(failure.reason.contains("runs no agent"), "{}", failure.reason);
}

fn mark(pane: &str) -> request::Payload {
    request::Payload::MarkUnread(MarkUnread { pane_id: pane.to_string(), ..MarkUnread::default() })
}

/// The pane's state as `muster window` reads it, and whether it could be marked unread.
fn state_of(pane: &str) -> Option<(String, bool)> {
    let Some(response::Payload::Window(window)) =
        answer(request::Payload::ReadWindow(ReadWindow::default())).payload
    else {
        return None;
    };
    window
        .panes
        .into_iter()
        .find(|listed| listed.pane_id == pane)
        .map(|listed| (listed.state, listed.markable_unread))
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
