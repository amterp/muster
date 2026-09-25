//! What the shell is sent when the window is republished, against a real daemon.
//!
//! Every republish is main-thread work in the shell - a forced layout per region, two sidebar
//! diffs, the badges redrawn - and many of them change nothing: a focus request for the pane
//! that already has the keyboard, a daemon echoing an arrangement the window already holds, a
//! relabel to the same label. So a view or a roster the shell already has is not sent again.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use herdr_harness::{Daemon, until};
use muster::proto::{
    AdjustFontSize, Event, FocusPane, OpenWindow, Request, Response, Startup, ViewChanged, event,
    request, response,
};
use prost::Message;

#[test]
fn a_republish_that_changes_nothing_sends_the_shell_nothing() {
    let _turn = muster::testing::fresh_session();
    // Nothing here starts a bridge, so every pane would become an untypeable one, which opens
    // the roster and republishes on its own schedule. Switched off so that every event counted
    // below was caused by this test.
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    muster::ffi::muster_set_event_callback(Some(note));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to open onto a pane",
        || keyboard().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    settle();
    let (daemon_id, pane_id) = keyboard().expect("the window opened onto a pane");
    let (views, rosters) = counts();

    // Twice, since the first could be the one that settles something the daemon had not yet
    // said. Neither moves the keyboard: it is already there.
    for _ in 0..2 {
        assert_ok(&answer(request::Payload::FocusPane(FocusPane {
            daemon_id: daemon_id.clone(),
            pane_id: pane_id.clone(),
        })));
    }
    // Then something that does change the view and not the roster, so the counts below are read
    // once everything before it has been published.
    assert_ok(&answer(request::Payload::AdjustFontSize(AdjustFontSize {
        change: "larger".to_string(),
    })));
    until(
        "the larger text to reach the shell",
        || latest_view().is_some_and(|view| format!("{view:?}").contains("font_size_offset: 1")),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    settle();

    let (views_after, rosters_after) = counts();
    assert_eq!(
        (views_after - views, rosters_after - rosters),
        (1, 0),
        "two focus requests that moved nothing and one text size change should reach the shell \
         as one view and no roster"
    );
}

/// Waits until nothing has been published for a while, so a count read next is not racing a
/// daemon event still on its way.
fn settle() {
    let quiet = Duration::from_millis(400);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = counts();
    let mut since = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        let now = counts();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return;
        }
    }
    panic!("the window never stopped republishing: {last:?} after ten seconds");
}

/// The daemon and pane the keyboard is on, as the published view names them.
fn keyboard() -> Option<(String, String)> {
    let view = latest_view()?;
    view.regions
        .into_iter()
        .find(|region| region.region_id == view.focused_region)
        .filter(|region| !region.pane_id.is_empty())
        .map(|region| (region.daemon_id, region.pane_id))
}

struct Seen {
    view: Option<ViewChanged>,
    views: usize,
    rosters: usize,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen { view: None, views: 0, rosters: 0 });

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    let mut seen = SEEN.lock().expect("a panicking test poisoned the events");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            seen.views += 1;
            seen.view = Some(view);
        }
        Some(event::Payload::RosterChanged(_)) => seen.rosters += 1,
        _ => {}
    }
}

fn counts() -> (usize, usize) {
    let seen = SEEN.lock().expect("a panicking test poisoned the events");
    (seen.views, seen.rosters)
}

fn latest_view() -> Option<ViewChanged> {
    SEEN.lock().expect("a panicking test poisoned the events").view.clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Made(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}
