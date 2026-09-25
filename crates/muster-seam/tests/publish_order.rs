//! Publishes from several threads reach the shell in the order the core settled them.
//!
//! The core remembers the last view it sent so that it need not send it again, and the shell
//! holds the last one it was given. If two publishes settle in one order and reach the shell in
//! the other, the shell holds the older view while the core believes the newer one arrived, and
//! nothing ever corrects it: every later publish computes the same view the core remembers and
//! sends nothing. Publishes run on daemon threads, the main thread and the watchdog's.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use herdr_harness::{Daemon, until};
use muster::proto::{
    AdjustFontSize, Event, OpenWindow, Request, Response, Startup, ViewChanged, event, request,
    response,
};
use prost::Message;

#[test]
fn the_last_view_the_shell_is_sent_is_the_one_the_core_settled_last() {
    let _turn = muster::testing::fresh_session();
    // SAFETY: nothing else in this process reads the environment concurrently. This runs
    // before the daemon is started and before any pane opens, which is when the core reads it.
    unsafe { std::env::set_var("MUSTER_TYPEABLE_DEADLINE_MS", "0") };

    let daemon = Daemon::start();
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
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));
    until(
        "the window to open onto a pane",
        || latest_view().is_some_and(|view| view.regions.iter().any(|r| !r.pane_id.is_empty())),
        || format!("the last view the core published: {:?}", latest_view()),
    );
    settle();

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
    for thread in threads {
        thread.join().expect("a publishing thread");
    }
    settle();

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

/// Waits until nothing has been published for a while.
fn settle() {
    let quiet = Duration::from_millis(400);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = count();
    let mut since = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        let now = count();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return;
        }
    }
    panic!("the window never stopped republishing");
}

/// Views received, the last of them, and how many were the same as the one before.
static SEEN: Mutex<(usize, Option<ViewChanged>, usize)> = Mutex::new((0, None, 0));

extern "C" fn note(bytes: *const u8, len: usize) {
    // SAFETY: the core guarantees `len` readable bytes for the duration of this call, which
    // is the contract in include/muster.h.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let event = Event::decode(bytes).expect("the core emits events this build can decode");
    if let Some(event::Payload::ViewChanged(view)) = event.payload {
        let mut seen = SEEN.lock().expect("a panicking test poisoned the events");
        seen.0 += 1;
        if seen.1.as_ref() == Some(&view) {
            seen.2 += 1;
        }
        seen.1 = Some(view);
    }
}

fn count() -> usize {
    SEEN.lock().expect("a panicking test poisoned the events").0
}

fn repeats() -> usize {
    SEEN.lock().expect("a panicking test poisoned the events").2
}

fn latest_view() -> Option<ViewChanged> {
    SEEN.lock().expect("a panicking test poisoned the events").1.clone()
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
