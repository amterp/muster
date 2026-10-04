//! Publishes from several threads reach the shell in the order the core settled them.
//!
//! The core remembers the last view it sent so that it need not send it again, and the shell
//! holds the last one it was given. If two publishes settle in one order and reach the shell in
//! the other, the shell holds the older view while the core believes the newer one arrived, and
//! nothing ever corrects it: every later publish computes the same view the core remembers and
//! sends nothing. Publishes run on daemon threads, the main thread and the watchdog's.

use std::sync::Mutex;
use std::time::Duration;

use muster::proto::{
    AdjustFontSize, Event, OpenWindow, Request, Response, RosterChanged, Startup, ViewChanged,
    ViewNode, event, request, response, view_node,
};
use muster_harness::{Daemon, until};
use prost::Message;

#[test]
fn the_last_view_the_shell_is_sent_is_the_one_the_core_settled_last() {
    let _turn = muster::testing::fresh_session();
    muster::testing::set_typeable_deadline(Duration::ZERO);

    let daemon = Daemon::start_built();
    let state = daemon.muster_config().with_file_name("window.toml");
    muster::ffi::muster_set_event_callback(Some(note));
    // With a run log, as every shipped build has one: its `view.region` lines are written
    // between settling a view and sending it, which is most of the time the two can be apart.
    let log = daemon.muster_config().with_file_name("run.jsonl");
    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        log_path: log.to_string_lossy().into_owned(),
        ..Startup::default()
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow::default())));
    // Everything opening publishes: the view and roster that add the tab, and the ones that add
    // the pane's record and its socket, which can come separately and in either order.
    until("the window to finish opening onto a pane", opened, || {
        format!("the last view: {:?}\nthe last roster: {:?}", latest_view(), latest_roster())
    });
    // Opening a window forgets what the shell was sent, on purpose, so the view it opens onto
    // can arrive twice. Counted from here.
    SEEN.lock().expect("a panicking test poisoned the events").2 = 0;

    // Each thread leaves the size where it found it, so once all are done the window is back
    // at zero - and the last view sent had better say so.
    let threads: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..300 {
                    for change in ["larger", "smaller"] {
                        assert_ok(&answer(request::Payload::AdjustFontSize(AdjustFontSize {
                            change: change.to_string(),
                        })));
                    }
                }
            })
        })
        .collect();
    // Joined is enough: a font size change publishes before its dispatch returns, and publishes
    // settle and send one at a time, so every view these changes caused has been sent. A publish
    // from another thread after this one settles the current view, which changes nothing below.
    for thread in threads {
        thread.join().expect("a publishing thread");
    }

    // The core never sends the view it last sent, so in order the shell never receives one
    // twice running. Out of order it does: settled S1, settled S2, sent S2, sent S1 - and the
    // next publish that settles S1 again is not a repeat to the core, which remembers S2.
    assert_eq!(
        repeats(),
        0,
        "the shell was sent the view it already held, so a publish reached it after one the \
         core settled later"
    );
    let last = latest_view().map(|view| format!("{view:?}")).unwrap_or_default();
    assert!(
        !last.contains("font_size_offset: 1") && !last.contains("font_size_offset: -1"),
        "every change was undone, and the shell was left holding an older view: {last}"
    );
}

/// Whether the window has finished opening onto its pane: the view gives the pane a socket for
/// its bridge, and the roster lists it.
fn opened() -> bool {
    let (Some(view), Some(roster)) = (latest_view(), latest_roster()) else { return false };
    let Some(pane) = view.regions.iter().find(|region| !region.pane_id.is_empty()) else {
        return false;
    };
    let socket = view
        .regions
        .iter()
        .filter_map(|region| region.root.as_ref())
        .any(|root| has_socket(root, &pane.pane_id));
    let listed =
        roster.tabs.iter().flat_map(|tab| &tab.panes).any(|row| row.pane_id == pane.pane_id);
    socket && listed
}

fn has_socket(node: &ViewNode, pane: &str) -> bool {
    match &node.node {
        Some(view_node::Node::Pane(leaf)) => {
            leaf.pane_id == pane && !leaf.link_socket_path.is_empty()
        }
        Some(view_node::Node::Split(split)) => {
            split.first.iter().chain(split.second.iter()).any(|child| has_socket(child, pane))
        }
        None => false,
    }
}

/// Views received, the last of them, and how many were the same as the one before.
static SEEN: Mutex<(usize, Option<ViewChanged>, usize)> = Mutex::new((0, None, 0));

/// The last roster received.
static ROSTER: Mutex<Option<RosterChanged>> = Mutex::new(None);

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    match event.payload {
        Some(event::Payload::ViewChanged(view)) => {
            let mut seen = SEEN.lock().expect("a panicking test poisoned the events");
            seen.0 += 1;
            if seen.1.as_ref() == Some(&view) {
                seen.2 += 1;
            }
            seen.1 = Some(view);
        }
        Some(event::Payload::RosterChanged(roster)) => {
            *ROSTER.lock().expect("a panicking test poisoned the roster") = Some(roster);
        }
        _ => {}
    }
}

fn latest_roster() -> Option<RosterChanged> {
    ROSTER.lock().expect("a panicking test poisoned the roster").clone()
}

fn repeats() -> usize {
    SEEN.lock().expect("a panicking test poisoned the events").2
}

fn latest_view() -> Option<ViewChanged> {
    SEEN.lock().expect("a panicking test poisoned the events").1.clone()
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
