//! Which daemons the window asks to hand their panes to this build's daemon.
//!
//! Only the one this install manages, found at its own socket. A socket named in the config is
//! somebody's own daemon, run how and for what they chose, and replacing it is not Muster's call
//! however old it is (MIP-3, section 10). The daemon asked is `older_daemon.rs`, a binary of its
//! own because it needs a scratch home; this one needs nothing but the config.

use std::time::Duration;

use muster::proto::{OpenWindow, ReadWindow, Request, Response, Startup, request, response};
use muster_harness::requests::{create, in_new_tab, make};
use muster_harness::{Daemon, built_daemon, until};
use prost::Message;

/// Longer than the handoff of an older daemon takes once the window follows it: the one this
/// install manages is handed over in well under half a second, daemon start included.
const LONG_ENOUGH_TO_HAVE_ASKED: Duration = Duration::from_secs(2);

#[test]
fn an_older_daemon_at_a_socket_somebody_named_is_not_asked() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let before = daemon.connect().welcome().instance;

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    until(
        "the window to follow the daemon it was pointed at",
        || listed_panes() == 1,
        || format!("the window lists {} panes", listed_panes()),
    );
    std::thread::sleep(LONG_ENOUGH_TO_HAVE_ASKED);

    assert_eq!(
        daemon.connect().welcome().instance,
        before,
        "the window asked a daemon at a socket the config named to hand its panes over"
    );
}

fn listed_panes() -> usize {
    match answer(request::Payload::ReadWindow(ReadWindow {})).payload {
        Some(response::Payload::Window(window)) => window.panes.len(),
        _ => 0,
    }
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
