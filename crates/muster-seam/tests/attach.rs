//! What attaching settles, against a real daemon.
//!
//! Attaching is where composition meets a backend: the core is handed a pane id and has to
//! turn it into a region showing that pane's tab, with the keyboard pointed at it. Only the
//! daemon knows which tab that is, so this is the one part of composition a recorded case
//! cannot judge - `composition.json` covers everything downstream of the answer, and this
//! covers getting one.
//!
//! The refusals matter as much as the success. A window attached to a pane no daemon holds
//! renders nothing and ignores the keyboard, which is indistinguishable from every other
//! way this can go wrong and is the symptom that has cost this project the most time.
//!
//! One test in this binary, on purpose. The seam holds the session in a process global and
//! this points the whole process at a scratch daemon through the environment; a second test
//! here would race both.

use herdr_harness::Daemon;
use muster::proto::{AttachPane, Paste, Request, Response, Startup, request, response};
use prost::Message;
use serde_json::{Value, json};

#[test]
fn attaching_places_a_pane_where_the_keyboard_can_find_it() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "attach", "focus": true }));
    let first = only_pane(&daemon);
    daemon.call("pane.split", &json!({ "pane_id": first, "direction": "right" }));
    let second = panes(&daemon)
        .into_iter()
        .find(|pane| pane != &first)
        .expect("the split gives this tab a second pane");

    // The core discovers its daemon the way a person's would, from the environment, so this
    // is the only way to point it at a scratch one.
    //
    // SAFETY: this binary holds one test, so the only other thread alive is the harness's
    // own, which reads no environment. The module docs say why it stays that way.
    unsafe { std::env::set_var("HERDR_SOCKET_PATH", daemon.socket_path()) };
    assert_ok(&answer(request::Payload::Startup(Startup::default())));

    // Before any attach, so this is the state a window is in on the way up rather than one
    // it fell back to.
    let reason = refusal(request::Payload::Paste(Paste { text: "hello".to_string() }));
    assert!(
        reason.contains("no pane has this window's keyboard"),
        "input with nothing attached should say so, and said: {reason}"
    );

    let reason = refusal(request::Payload::AttachPane(AttachPane { pane_id: "w9:p9".to_string() }));
    assert!(
        reason.contains("w9:p9") && reason.contains("herdr pane list"),
        "a pane no daemon holds should be refused by name, and was refused with: {reason}"
    );

    let one = attach(&first);
    assert!(
        std::path::Path::new(&one.control_socket_path).exists(),
        "the bridge's socket is bound before attach returns, and {} is not there",
        one.control_socket_path
    );
    assert!(
        one.server_encoded,
        "the core found no daemon to encode against, which means the attach that just \
         succeeded was answered by something other than the daemon this test started"
    );

    // A second pane in the same tab. Two things are being asserted at once because they are
    // the same mistake: a socket per process rather than per pane would hand back the path
    // it already gave out, and one bridge would be talking for both panes.
    let two = attach(&second);
    assert_ne!(
        one.control_socket_path, two.control_socket_path,
        "each pane dials the core on its own socket, and both panes were given one path"
    );

    // The keyboard follows the pane just attached, which is the whole of composition doing
    // its job: a region for the tab, a view-local cursor in it, and a lookup that found the
    // attachment behind it.
    //
    // Asserted on the panes rather than on the answer, because the answer is `ok` either
    // way - the seam reports that it found somewhere to send, not where. Both panes run a
    // shell, so text sent to one and not the other is visible on exactly one screen, and
    // the wrong-pane bug is the one that looks like nothing at all from here.
    //
    // A paste rather than a keystroke, because it is the intent the core hands to the
    // daemon to encode. Everything else leaves over the pane's own socket, which needs a
    // bridge process on the far end - that path has its own test, and standing one up here
    // would make this one about two things.
    let typed = "muster-attached-here";
    assert_ok(&answer(request::Payload::Paste(Paste { text: typed.to_string() })));
    until("the text to appear in the pane that has the keyboard", || {
        screen(&daemon, &second).contains(typed)
    });
    assert!(
        !screen(&daemon, &first).contains(typed),
        "the keyboard should follow the pane just attached, and the text landed in {first} \
         as well as, or instead of, {second}"
    );
}

fn answer(payload: request::Payload) -> Response {
    let request = Request { payload: Some(payload) };
    Response::decode(muster::dispatch(&request.encode_to_vec()).as_slice())
        .expect("the core answers every request with a decodable response")
}

fn assert_ok(response: &Response) {
    match &response.payload {
        Some(response::Payload::Ok(_) | response::Payload::Attached(_)) => {}
        Some(response::Payload::Failure(failure)) => panic!("the core refused: {}", failure.reason),
        None => panic!("the core answered with no payload"),
    }
}

/// The reason a request was refused, or a panic saying it was not.
fn refusal(payload: request::Payload) -> String {
    match answer(payload).payload {
        Some(response::Payload::Failure(failure)) => failure.reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn attach(pane: &str) -> muster::proto::Attached {
    match answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() })).payload {
        Some(response::Payload::Attached(attached)) => attached,
        other => panic!("expected an attachment for {pane}, got {other:?}"),
    }
}

fn panes(daemon: &Daemon) -> Vec<String> {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no panes in {snapshot}"))
        .iter()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn only_pane(daemon: &Daemon) -> String {
    let panes = panes(daemon);
    assert_eq!(panes.len(), 1, "a fresh workspace should hold exactly one pane: {panes:?}");
    panes[0].clone()
}

/// What a pane is showing, asked of the daemon that renders it.
///
/// A daemon renders every pane whether or not anything is attached to it, which is what
/// makes this a usable oracle here: no surface, no bridge, and a screen to read anyway.
fn screen(daemon: &Daemon, pane: &str) -> String {
    let read = daemon.call("pane.read", &json!({ "pane_id": pane, "source": "visible" }));
    read.get("read")
        .and_then(|read| read.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a pane read carries its text under `read`: {read}"))
        .to_string()
}

/// Polls a condition rather than sleeping on it.
///
/// herdr answers in under a millisecond, so a sleep long enough to be safe makes the suite
/// unpleasant and one short enough to be pleasant is flaky on a loaded machine.
fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("timed out after 15s waiting for {what}");
}
