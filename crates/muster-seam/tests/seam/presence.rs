//! The person at the desk, as a window tells the daemon on its machine (MIP-10, section 4):
//! whether Muster is in front, when somebody last used it, and what it showed them.

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

use muster::proto::{
    AttentionShown, AttentionShownAs, KeyDown, KeyEvent, OpenWindow, Request, Response, Startup,
    WindowFocus, request, response,
};
use muster_daemon_proto as daemon_proto;
use muster_harness::requests::{create, in_new_tab, make, snapshot, subscribe_request, until_text};
use muster_harness::{Control, Daemon, until};
use prost::Message;

/// A window reports to its daemon as soon as it attends, again when it gains the OS's focus, and
/// at once when somebody first types; and passes on what it showed them.
#[test]
fn a_window_tells_its_daemon_of_the_person_at_the_desk() {
    let _turn = muster::testing::fresh_session();
    let daemon = Daemon::start_built();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    until_text(&mut control, "p1", "$");
    for payload in [
        request::Payload::Startup(Startup {
            config_path: daemon.muster_config().to_string_lossy().into_owned(),
            ..Startup::default()
        }),
        request::Payload::OpenWindow(OpenWindow::default()),
    ] {
        assert_ok(&answer(payload));
    }

    let mut client = daemon.connect();
    let reported = until_presence(&mut client, "the window to report", |presence| presence.known);
    assert!(reported.attended, "{reported:?}");
    assert!(!reported.in_front, "no window has the OS's focus yet: {reported:?}");

    assert_ok(&answer(request::Payload::WindowFocus(WindowFocus { focused: true })));
    until_presence(&mut client, "Muster to be in front", |presence| presence.in_front);

    let before = SystemTime::now().duration_since(UNIX_EPOCH).expect("after 1970").as_millis();
    press("KeyA", "a");
    let used = until_presence(&mut client, "the keystroke to be reported", |presence| {
        presence.last_input_ms.is_some()
    });
    let at = used.last_input_ms.expect("waited for");
    assert!(u128::from(at) >= before, "used at {at}, typed after {before}");

    let subscribed = control.ask(subscribe_request());
    assert_eq!(subscribed.outcome(), daemon_proto::Outcome::Done);
    assert_ok(&answer(request::Payload::AttentionShown(AttentionShown {
        daemon_id: "local".to_string(),
        pane_id: "p1".to_string(),
        shown_as: AttentionShownAs::Notification.into(),
        why: "blocked".to_string(),
        ..AttentionShown::default()
    })));
    let shown = loop {
        if let Some(daemon_proto::event::Event::Shown(shown)) = control.next_event().event {
            break shown;
        }
    };
    assert_eq!(shown.about, Some(daemon_proto::shown::About::Pane("p1".to_string())));
    assert_eq!(
        (shown.shown_as(), shown.why.as_str()),
        (daemon_proto::ShownAs::Notification, "blocked")
    );
}

fn until_presence(
    client: &mut Control,
    what: &str,
    ready: impl Fn(&daemon_proto::Presence) -> bool,
) -> daemon_proto::Presence {
    let last = RefCell::new(daemon_proto::Presence::default());
    until(
        what,
        || {
            *last.borrow_mut() = snapshot(client).presence.unwrap_or_default();
            ready(&last.borrow())
        },
        || format!("the daemon has {:?}", last.borrow()),
    );
    last.into_inner()
}

fn press(key: &str, text: &str) {
    let down = KeyDown {
        key: Some(KeyEvent {
            action: "press".to_string(),
            key: key.to_string(),
            text: text.to_string(),
            ..KeyEvent::default()
        }),
        ..KeyDown::default()
    };
    assert_ok(&answer(request::Payload::KeyDown(down)));
}

fn answer(payload: request::Payload) -> Response {
    let bytes = Request::new(payload).encode_to_vec();
    Response::decode(muster::dispatch(&bytes).as_slice()).expect("a response this build knows")
}

fn assert_ok(response: &Response) {
    if let Some(response::Payload::Failure(failure)) = &response.payload {
        panic!("the core refused: {}", failure.reason);
    }
}
