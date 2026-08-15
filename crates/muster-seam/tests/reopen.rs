//! A window coming back the way it was left, against a real daemon.
//!
//! The arrangement is the one thing Muster owns and no daemon can answer: which of its tabs
//! this window was showing, in what order, at what widths. `composition-saved.json` pins the
//! rules for reading a saved arrangement back; what needs a daemon is the round trip - that a
//! window settles, writes, and that a second one opening onto the same session shows the same
//! thing.
//!
//! Two processes cannot be had here, because the seam holds one session per process. So the
//! file is the seam: this drives a window, reads what it wrote, and asserts on that - which is
//! also the only part a second process would have to go on.

use std::sync::Mutex;

use herdr_harness::Daemon;
use muster::proto::{
    CreateTab, Event, OpenWindow, Request, Response, Startup, ViewChanged, event, request, response,
};
use prost::Message;

#[test]
fn a_window_writes_down_what_it_is_showing() {
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    muster::ffi::muster_set_event_callback(Some(note_view));
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    until(
        "the window to open onto a workspace",
        || tab_of_first_region().is_some(),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let first = tab_of_first_region().expect("just waited for it");

    // Somewhere to move to, so what is written down is a choice rather than the only thing
    // there was. A new tab is what a person reaches for several times an hour.
    assert_ok(&answer(request::Payload::CreateTab(CreateTab::default())));
    until(
        "the window to move onto the tab it asked for",
        || tab_of_first_region().is_some_and(|tab| tab != first),
        || format!("the region still shows {first}"),
    );
    let second = tab_of_first_region().expect("just waited for it");

    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the window wrote no arrangement to {}: {e}", state.display()));

    // The tab it ended on, not the one it started on. A file that recorded the first would be
    // one written at open and never again, which is the failure that looks most like working.
    assert!(
        written.contains(&second),
        "the arrangement does not name the tab the window was showing ({second}):\n{written}"
    );
    assert!(
        !written.contains(&first),
        "the arrangement still names the tab the window moved off ({first}), so it was \
         written once and never updated:\n{written}"
    );
    // Intent, not observation. The tab somebody chose is written; how that tab was arranged
    // is not, because it is the daemon's to say and it will have moved on. A file carrying a
    // pane tree is one that can disagree with the session it reopens onto.
    for daemon_truth in ["columns", "rows", "ratio", "split"] {
        assert!(
            !written.contains(daemon_truth),
            "the arrangement wrote down {daemon_truth:?}, which is the daemon's answer and \
             not this window's:\n{written}"
        );
    }
    // Equal widths are a default nobody minds losing on restart; a width somebody dragged is
    // not, which is what turned this from tidiness into something users notice.
    assert!(written.contains("weight"), "no width was written down at all:\n{written}");

    // And the whole point of writing it: a window opening onto this session again shows the
    // tab it was left on. Read through the same rules the core uses, against the tabs this
    // daemon actually holds - which is the check that makes a saved region a wish.
    let saved = muster_core::composition::saved::from_toml(&written)
        .expect("the core can read back what it just wrote");
    let restorable = saved.restorable(|_, tab| tab.as_str() == second);
    assert_eq!(
        restorable.regions.len(),
        1,
        "reopening onto this session shows {} regions rather than the one it was left with",
        restorable.regions.len()
    );
    assert_eq!(restorable.regions[0].tab.as_str(), second);
}

fn tab_of_first_region() -> Option<String> {
    let view = latest_view()?;
    let region = view.regions.first()?;
    // A region with no tree is one the daemon has not described yet, which is not the state
    // this is waiting for.
    region.root.as_ref()?;
    Some(region.tab_id.clone())
}

static VIEW: Mutex<Option<ViewChanged>> = Mutex::new(None);

extern "C" fn note_view(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ViewChanged(view)) = event.payload {
        *VIEW.lock().expect("a panicking test poisoned the view") = Some(view);
    }
}

fn latest_view() -> Option<ViewChanged> {
    VIEW.lock().expect("a panicking test poisoned the view").clone()
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request { payload: Some(payload) }.encode_to_vec();
    let reply = muster::dispatch(&bytes);
    Response::decode(reply.as_slice()).expect("the core answers with a response this build knows")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_)) => {}
        other => panic!("expected the core to accept this, and it answered {other:?}"),
    }
}

fn until(what: &str, mut ready: impl FnMut() -> bool, on_failure: impl FnOnce() -> String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}. {}", on_failure());
}
