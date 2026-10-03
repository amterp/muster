//! One app per install per Muster home (mip/0006-one-process.md, section 5).
//!
//! The first launch holds a lock; a second, from the Dock, `open -n` or an older `muster`, finds
//! it held and hands what it was launched to do to the app holding it. Both halves run in this one
//! test process, which works because the lock is taken on a descriptor of its own each time.

use std::sync::Mutex;

use muster::proto::{
    AskForWindow, ClaimApp, Event, OpenWindow, Request, Response, Startup, event, request, response,
};
use muster_harness::{Daemon, until};
use prost::Message;

/// A second launch is handed to the app already running, which is asked for the window, and the
/// launch is told it has nothing left to do.
#[test]
fn a_second_launch_hands_its_window_to_the_running_app() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let home = daemon.root().join("home");
    let socket = daemon.root().join("command.sock");
    let socket = socket.to_string_lossy().into_owned();

    let first = claim(&home.to_string_lossy(), &socket, AskForWindow::default());
    assert!(first.claimed, "the first launch was not given the app");
    assert_eq!(
        first.state_directory,
        home.join("state").join(muster_daemon_proto::install::INSTALL).to_string_lossy(),
        "the app keeps its state somewhere other than its install's own directory"
    );
    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            command_socket_path: socket.clone(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        answer(&Request::new(payload));
    }

    let second = claim(
        &home.to_string_lossy(),
        &daemon.root().join("another.sock").to_string_lossy(),
        AskForWindow { any: true, ..AskForWindow::default() },
    );
    assert!(!second.claimed, "a second launch was given the app as well");
    until(
        "the running app to be asked for any window",
        || asked().iter().any(|(any, fresh)| *any && !*fresh),
        || format!("the app was asked for {:?}", asked()),
    );

    let elsewhere = daemon.root().join("elsewhere");
    let other_home = claim(&elsewhere.to_string_lossy(), &socket, AskForWindow::default());
    assert!(other_home.claimed, "a launch under another home was handed to this one");
}

fn claim(home: &str, socket: &str, ask: AskForWindow) -> muster::proto::AppClaim {
    match answer(&Request::new(request::Payload::ClaimApp(ClaimApp {
        home: home.to_string(),
        command_socket_path: socket.to_string(),
        ask: Some(ask),
    })))
    .payload
    {
        Some(response::Payload::AppClaim(claim)) => claim,
        other => panic!("claiming the app answered {other:?}"),
    }
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(event);
}

fn forget_events() {
    EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// Every window the app was asked for, as whether any would do and whether it was to be fresh.
fn asked() -> Vec<(bool, bool)> {
    EVENTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|event| match &event.payload {
            Some(event::Payload::ReopenWindow(reopen)) => Some((reopen.any, reopen.fresh)),
            _ => None,
        })
        .collect()
}

fn answer(request: &Request) -> Response {
    let reply = muster::dispatch(&request.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}
