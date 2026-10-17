//! A pane whose daemon stops answering reads `unreachable`, against a real daemon.
//!
//! A devenv rebooted, and for two days the window listed its agents as `idle`, `done` and
//! blocked on the person, because each pane kept what its daemon last said and only the
//! machine's line said otherwise (kan a_2cJ6cgiLw). What the daemon last said is kept, beside
//! the state rather than as it, so that nothing reading a state takes the pane for a live one.

use muster::proto::{
    OpenWindow, ReadAsking, ReadWindow, Request, Response, Startup, request, response,
};
use muster_harness::requests::{among_helpers, create, helper, in_new_tab, make, snapshot};
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

/// A quiet helper whose maker's machine stops answering asks for the person, who is the only one
/// left who can act, and goes quiet again once the maker can be reached (MIP-7, section 4).
#[test]
fn a_quiet_helper_asks_for_the_person_while_its_maker_cannot_be_reached() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let laptop = Daemon::start_detecting();
    let mut devenv = Daemon::start_detecting();
    make(&mut devenv.connect(), create("p1devenv00", in_new_tab("t1devenv00")));
    devenv.run_agent("p1devenv00");
    make(&mut laptop.connect(), create("p1", in_new_tab("t1")));
    let quiet = muster_daemon_proto::pane_request::Create {
        quiet: true,
        ..helper("p2", "p1devenv00", among_helpers("t2", "its maker's helpers"))
    };
    make(&mut laptop.connect(), quiet);
    laptop.run_agent_at_a_dialog("p2");
    let config = laptop.muster_config_naming("laptop", &[("devenv", &devenv)]);
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: config.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to see the helper blocked, and quiet",
        || pane("p2").is_some_and(|(state, _)| state == "blocked") && pane("p1devenv00").is_some(),
        || format!("p2 reads {:?}, asking {:?}", pane("p2"), read_asking()),
    );
    assert!(read_asking().is_empty(), "a quiet helper asked: {:?}", read_asking());

    devenv.kill();
    until(
        "the helper to ask for the person once its maker cannot be reached",
        || read_asking().pane_id == "p2",
        || format!("asking {:?}", read_asking()),
    );

    // A restart ends the maker's agent, and a maker with no agent hears nothing either.
    devenv.restart();
    until(
        "the devenv to bring the maker's pane back",
        || snapshot(&mut devenv.connect()).panes.iter().any(|pane| pane.pane == "p1devenv00"),
        || "no pane p1devenv00 yet".to_string(),
    );
    devenv.run_agent("p1devenv00");
    until(
        "the helper to go quiet again once its maker answers",
        || read_asking().is_empty(),
        || format!("asking {:?}", read_asking()),
    );
}

/// A quiet helper whose maker's agent has left its pane asks for the person, since nothing there
/// hears it, and goes quiet again once an agent is back (MIP-7, section 4).
#[test]
fn a_quiet_helper_asks_for_the_person_while_its_maker_has_no_agent() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_detecting();
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    daemon.run_agent("p1");
    let quiet = muster_daemon_proto::pane_request::Create {
        quiet: true,
        ..helper("p2", "p1", among_helpers("t2", "p1's helpers"))
    };
    make(&mut daemon.connect(), quiet);
    daemon.run_agent_at_a_dialog("p2");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to see the helper blocked, and its maker's agent",
        || pane("p2").is_some_and(|(state, _)| state == "blocked") && pane("p1").is_some(),
        || format!("p2 reads {:?}, asking {:?}", pane("p2"), read_asking()),
    );
    assert!(read_asking().is_empty(), "a quiet helper asked: {:?}", read_asking());

    daemon.type_into("p1", "quit");
    until(
        "the helper to ask for the person once its maker's agent has gone",
        || read_asking().pane_id == "p2",
        || format!("asking {:?}", read_asking()),
    );

    daemon.run_agent("p1");
    until(
        "the helper to go quiet again once an agent is back in its maker's pane",
        || read_asking().is_empty(),
        || format!("asking {:?}", read_asking()),
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
