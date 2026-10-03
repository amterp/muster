//! Several windows in one process (MIP-6).
//!
//! What a window is to the core: a name the events it is sent carry, and a target a request can
//! name. A shell holding one window ignores both, so these are the tests that notice if either
//! stops being true.

use std::sync::Mutex;

use muster::proto::{
    Event, OpenWindow, Request, Response, Startup, ToggleSidebar, event, request, response,
};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn what_a_window_is_sent_names_it() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(std::time::Duration::ZERO);
    let daemon = Daemon::start_built();
    let state = daemon.muster_config().with_file_name("window-7.toml");

    forget_events();
    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(&Request::new(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        ..Startup::default()
    }))));
    assert_ok(&answer(&Request::new(request::Payload::OpenWindow(OpenWindow::default()))));

    until(
        "the window to be told what it shows",
        || windows_of(|payload| matches!(payload, event::Payload::ViewChanged(_))).is_some(),
        || "no ViewChanged arrived".to_string(),
    );
    assert_eq!(
        windows_of(|payload| matches!(payload, event::Payload::ViewChanged(_))).as_deref(),
        Some("window-7"),
        "a view was sent without the name of the window it is for, so a shell holding two could \
         not tell which one to draw it in"
    );
}

#[test]
fn a_request_for_a_window_this_process_has_not_got_is_refused() {
    let _turn = muster::testing::fresh_session();
    let response = answer(
        &Request::new(request::Payload::ToggleSidebar(ToggleSidebar {})).for_window("window-99"),
    );
    match response.payload {
        Some(response::Payload::Failure(failure)) => assert!(
            failure.reason.contains("window-99"),
            "the refusal does not say which window it could not find: {}",
            failure.reason
        ),
        other => panic!(
            "a request for a window nobody opened was carried out somewhere instead: {other:?}"
        ),
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

/// The window the latest event of a kind was for, if one of that kind has arrived.
fn windows_of(kind: impl Fn(&event::Payload) -> bool) -> Option<String> {
    let events = EVENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    events
        .iter()
        .rev()
        .find(|event| event.payload.as_ref().is_some_and(&kind))
        .map(|event| event.window.clone())
}

fn answer(request: &Request) -> Response {
    let reply = muster::dispatch(&request.encode_to_vec());
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    assert!(
        !matches!(response.payload, Some(response::Payload::Failure(_))),
        "the core refused: {response:?}"
    );
}
