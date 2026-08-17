//! Sizing one pane's text, against a real daemon.
//!
//! The rules are pinned by `composition.json` with no socket in sight, and the round trip
//! through the file by `composition_saved.rs`. What needs a daemon is the part in between:
//! that a chord reaches the pane the keyboard is on and no other, that a pane a split makes
//! opens at the size of the pane it came from, and that the answer arrives on the pane in the
//! published view rather than on a message a shell would have to join against it.
//!
//! One test here so far, and no longer because a second could not be had: the seam's session
//! is reset between tests and they take their turns through `muster::testing::fresh_session`,
//! which is what the first line of each one is asking for.

use std::sync::Mutex;

use herdr_harness::{Daemon, until};
use muster::proto::{
    AdjustFontSize, Event, OpenWindow, Request, Response, SplitPane, Startup, ViewChanged,
    ViewNode, event, request, response, view_node,
};
use prost::Message;

#[test]
fn a_chord_sizes_one_pane_and_a_split_inherits_it() {
    let _turn = muster::testing::fresh_session();
    // Nothing here starts a bridge, so every pane is one that never becomes typeable - an
    // error, which opens the roster and republishes. Harmless to this test and noisy in its
    // log, so it is switched off rather than waited out.
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

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
        "the window to open onto a pane",
        || panes().len() == 1,
        || format!("the last view the core published: {:?}", latest_view()),
    );

    // A neighbour to leave alone. Sized before the split so that what follows is about a pane
    // that already had a size when the second one appeared.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "right".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    until(
        "the split to settle at two panes",
        || panes().len() == 2,
        || format!("the last view the core published: {:?}", latest_view()),
    );
    let sized = keyboard_pane().expect("the split took the keyboard, which is what it asked for");

    for _ in 0..2 {
        assert_ok(&answer(request::Payload::AdjustFontSize(AdjustFontSize {
            change: "larger".to_string(),
        })));
    }

    // The whole of what this reversed, and the assertion that fails under the old behaviour:
    // one number for the window would size both of these together.
    until(
        "the pane with the keyboard to be two points bigger, and its neighbour untouched",
        || panes().iter().all(|(pane, offset)| *offset == if *pane == sized { 2 } else { 0 }),
        || format!("the panes are {:?}", panes()),
    );

    // Written down for the next launch, and read back through the core's own rules rather than
    // by looking for a string, since that is the path the next launch takes.
    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the window wrote nothing to {}: {e}", state.display()));
    assert!(
        written.contains("[[pane]]"),
        "the file records no pane's text size at all:\n{written}"
    );
    let saved = muster_core::composition::saved::from_toml(&written)
        .expect("the core can read back what it just wrote");
    let remembered: Vec<i32> = saved.font_sizes.entries().map(|(_, offset)| offset).collect();
    assert_eq!(
        remembered,
        vec![2],
        "reopening would put the text back at the configured size, so the chord did not \
         survive the file:\n{written}"
    );

    // And a pane made from a pane somebody had grown opens where that one is. The alternative
    // is splitting a pane you can finally read and getting one you cannot.
    assert_ok(&answer(request::Payload::SplitPane(SplitPane {
        side: "down".to_string(),
        take_focus: true,
        ..SplitPane::default()
    })));
    until(
        "the pane the split made to open at the size of the pane it came from",
        || panes().len() == 3 && panes().iter().filter(|(_, offset)| *offset == 2).count() == 2,
        || format!("the panes are {:?}", panes()),
    );

    // Reset is the way back out, and it is one pane's way out. A reset that reached the window
    // would undo work on panes this person never touched.
    assert_ok(&answer(request::Payload::AdjustFontSize(AdjustFontSize {
        change: "reset".to_string(),
    })));
    let made = keyboard_pane().expect("the second split took the keyboard too");
    until(
        "the keyboard's pane to go back to the configured size, and the first one to stay",
        || {
            let panes = panes();
            panes.iter().any(|(pane, offset)| *pane == made && *offset == 0)
                && panes.iter().any(|(pane, offset)| *pane == sized && *offset == 2)
        },
        || format!("the panes are {:?}", panes()),
    );
}

/// Every pane the window is showing, as its name and how far its text is from the configured
/// size, in the order the tree lays them out.
fn panes() -> Vec<(String, i32)> {
    let Some(view) = latest_view() else { return Vec::new() };
    view.regions.iter().filter_map(|region| region.root.as_ref()).flat_map(leaves).collect()
}

fn leaves(node: &ViewNode) -> Vec<(String, i32)> {
    match &node.node {
        Some(view_node::Node::Pane(pane)) => vec![(pane.pane_id.clone(), pane.font_size_offset)],
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).flat_map(|child| leaves(child)).collect()
        }
        None => Vec::new(),
    }
}

/// The pane this window's keyboard feeds, as the published view names it.
fn keyboard_pane() -> Option<String> {
    let view = latest_view()?;
    let focused = view.focused_region.clone();
    view.regions
        .into_iter()
        .find(|region| region.region_id == focused)
        .map(|region| region.pane_id)
        .filter(|pane| !pane.is_empty())
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
