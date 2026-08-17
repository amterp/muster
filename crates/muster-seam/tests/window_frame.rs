//! A window coming back the size it was left, against a real daemon.
//!
//! The fit rules are pinned by `window-frame.json` and the file format by
//! `composition_saved.rs`. What needs a session is the round trip between them: that a shell
//! asking where to open is answered against the screens it reported, that saying where it
//! settled reaches the same file the arrangement does, and - the one that has broken before -
//! that a frame reported during launch is not thrown away by the restore that follows it.
//!
//! One test here so far, and no longer because a second could not be had: the seam's session
//! is reset between tests and they take their turns through `muster::testing::fresh_session`,
//! which is what the first line of each one is asking for.

use herdr_harness::Daemon;
use muster::proto::{
    OpenWindow, ReadWindowFrame, Request, Response, SetWindowFrame, Startup, WindowFrame,
    WindowRect, request, response,
};
use muster_core::composition::presentation::{Frame, Presentation};
use muster_core::composition::saved::{Saved, to_toml};
use prost::Message;

/// The laptop this test pretends to be running on, and the desk monitor it is not.
const LAPTOP: WindowRect = WindowRect { x: 0.0, y: 0.0, width: 1440.0, height: 875.0 };
const ON_A_MONITOR_THAT_IS_GONE: Frame =
    Frame { x: 3000.0, y: 200.0, width: 1200.0, height: 800.0 };

#[test]
fn a_window_comes_back_the_size_it_was_left() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start();
    let state = daemon.muster_config().with_file_name("window.toml");

    // A previous run, which quit with the window on a display this machine no longer has.
    std::fs::write(
        &state,
        to_toml(&Saved {
            presentation: Presentation::default()
                .with_frame(Some(ON_A_MONITOR_THAT_IS_GONE), false),
            ..Saved::default()
        }),
    )
    .expect("the scratch config directory is writable");

    assert_ok(&answer(request::Payload::Startup(Startup {
        config_path: daemon.muster_config().to_string_lossy().into_owned(),
        state_path: state.to_string_lossy().into_owned(),
        ..Startup::default()
    })));

    // Asked before the window is on screen, which is the whole reason this is a question and
    // not an event. The saved rectangle is nowhere near the only screen there is, so what comes
    // back is somewhere a person can reach rather than what the file said.
    let opening = frame_answer(&[LAPTOP]);
    let rect = opening.rect.expect("a saved rectangle should be answered with one");
    assert_eq!(
        (rect.width, rect.height),
        (1200.0, 800.0),
        "a window that fits the screen was resized to reach it"
    );
    assert!(
        rect.x >= LAPTOP.x && rect.x + rect.width <= LAPTOP.x + LAPTOP.width,
        "the window was opened off the side of the only screen there is: {rect:?}"
    );

    // Then where it actually opened, which in a real launch is reported before the window is
    // asked to open onto anything. This is the ordering that has gone wrong before: `open`
    // replaces the whole presentation from the file, so a wholesale restore would put the
    // window back on the monitor that is gone.
    assert_ok(&answer(request::Payload::SetWindowFrame(SetWindowFrame {
        frame: Some(WindowFrame { rect: Some(rect), full_screen: false }),
    })));
    assert_ok(&answer(request::Payload::OpenWindow(OpenWindow {})));

    let written = std::fs::read_to_string(&state)
        .unwrap_or_else(|e| panic!("the window wrote nothing to {}: {e}", state.display()));
    let saved = muster_core::composition::saved::from_toml(&written)
        .expect("the core can read back what it just wrote");
    let kept = saved.presentation.frame.expect("the file records no rectangle at all");
    assert_eq!(
        (kept.x, kept.y, kept.width, kept.height),
        (rect.x, rect.y, rect.width, rect.height),
        "the file holds the rectangle from before the restore rather than where the window \
         actually is:\n{written}"
    );

    // And the same file the arrangement goes in, beside the roster's own setting rather than in
    // one of its own - a second file is a second thing that can be lost or left behind.
    assert!(
        written.contains("[window]") && written.contains("sidebar"),
        "the frame did not land in the [window] table beside the rest of the chrome:\n{written}"
    );

    // Full-screen is remembered as itself, and the rectangle underneath it is the one to come
    // back out to. macOS reports a full-screen window's frame as the whole display, so a
    // window that wrote that down would leave full-screen the size of somebody's monitor.
    assert_ok(&answer(request::Payload::SetWindowFrame(SetWindowFrame {
        frame: Some(WindowFrame { rect: Some(rect), full_screen: true }),
    })));
    let reopening = frame_answer(&[LAPTOP]);
    assert!(reopening.full_screen, "quitting from full-screen was not remembered");
    assert_eq!(
        reopening.rect.map(|r| (r.width, r.height)),
        Some((rect.width, rect.height)),
        "full-screen overwrote the size the window comes back out to"
    );

    // A shell that could not enumerate displays is a bug here rather than a machine with no
    // monitors, and moving somebody's window over one would turn that bug into a lost
    // arrangement.
    assert_eq!(
        frame_answer(&[]).rect.map(|r| (r.x, r.y)),
        Some((rect.x, rect.y)),
        "reporting no screens moved the window"
    );
}

/// What the core says about where to open, given these screens.
fn frame_answer(screens: &[WindowRect]) -> WindowFrame {
    let response =
        answer(request::Payload::ReadWindowFrame(ReadWindowFrame { screens: screens.to_vec() }));
    match response.payload {
        Some(response::Payload::WindowFrame(frame)) => frame,
        other => panic!("expected a window frame, and the core answered {other:?}"),
    }
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
