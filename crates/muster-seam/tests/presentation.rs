//! Putting the roster away, and finding it still away next time.
//!
//! The rules for reading the file back are pinned by `composition_saved.rs`, and the chord
//! that dispatches this by `bindings.json`. What needs a real daemon is the round trip
//! between them: that asking for it changes what the core says, that the answer reaches the
//! same file the arrangement does, and that a window opening onto it starts where it left off.
//!
//! Asserted through the event rather than by reading the core's state, because the event is
//! the whole of what a shell has to go on - a window shows the list because it was told to,
//! and a core that changed its mind privately would look identical from inside.
//!
//! One test here so far, and no longer because a second could not be had: the seam's session
//! is reset between tests and they take their turns through `muster::testing::fresh_session`,
//! which is what the first line of each one is asking for.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    Event, OpenWindow, PresentationChanged, Request, Response, Startup, ToggleSidebar, event,
    request, response,
};
use prost::Message;

#[test]
fn putting_the_roster_away_is_remembered() {
    let _turn = muster::testing::fresh_session();
    // Nothing here starts a bridge, so the pane below is one that never becomes typeable -
    // and that is an error, which opens a roster this test has just put away. Switched off
    // rather than sized generously, because a deadline crossed on a loaded runner would fail
    // this test with an assertion about the roster and no hint that a watchdog moved it.
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    muster::ffi::muster_set_event_callback(Some(note_presentation));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    // Told without being asked, and before anything is toggled. A shell holding its own
    // default would agree with this by luck until the default moved.
    until(
        "the core to say what the window shows of itself",
        || latest().is_some(),
        || "no PresentationChanged arrived at all".to_string(),
    );
    assert!(
        latest().expect("just waited for it").sidebar,
        "a first launch opened with the roster hidden, so a pane nobody is showing has no \
         way to be found"
    );

    assert_ok(&answer(request::Payload::ToggleSidebar(ToggleSidebar {})));
    until(
        "the roster to be put away",
        || latest().is_some_and(|p| !p.sidebar),
        || format!("the core still says {:?}", latest()),
    );

    // The same file the arrangement goes in, written by the same settle. A second file, or a
    // second thing that has to remember to save, is a second thing that can forget.
    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the window wrote nothing to {}: {e}", state.display()));
    assert!(
        written.contains("[window]") && written.contains("sidebar = false"),
        "the file does not record that the roster was put away:\n{written}"
    );

    // And what a next launch would make of it. Read through the core's own rules rather than
    // by looking for a string, since that is the path the next launch takes.
    let saved = muster_core::composition::saved::from_toml(&written)
        .expect("the core can read back what it just wrote");
    assert!(
        !saved.presentation.sidebar,
        "reopening would put the roster back, so the decision did not survive the file"
    );

    // Back again, because a toggle that only works one way is a toggle nobody can undo.
    assert_ok(&answer(request::Payload::ToggleSidebar(ToggleSidebar {})));
    until(
        "the roster to come back",
        || latest().is_some_and(|p| p.sidebar),
        || format!("the core still says {:?}", latest()),
    );
}

static PRESENTATION: Mutex<Option<PresentationChanged>> = Mutex::new(None);

extern "C" fn note_presentation(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::PresentationChanged(presentation)) = event.payload {
        *PRESENTATION.lock().expect("a panicking test poisoned the presentation") =
            Some(presentation);
    }
}

fn latest() -> Option<PresentationChanged> {
    *PRESENTATION.lock().expect("a panicking test poisoned the presentation")
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

/// That the core accepted a request, whichever shape its acceptance took.
///
/// `Made` is an acceptance too: a request that creates a pane answers with the pane rather than
/// with a bare Ok, because the name was minted inside the call and a caller cannot learn it any
/// other way. Only `Failure` is a refusal.
fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
