//! Which pane a wheel moves, with a real daemon deciding.
//!
//! A scroll is the one input Muster addresses rather than focuses: it moves the pane the
//! pointer is over, because reading one agent's output while typing into another is the
//! ordinary thing to do in a window of fifteen. Everything else here means the pane with the
//! keyboard.
//!
//! Nothing short of a live daemon can judge that. The shell's own suite pins which ids leave
//! the view, and the seam answers `ok` whichever pane it found - so the failure this exists to
//! catch, a scroll routed to the keyboard pane, looks identical from every layer above. The
//! oracle is herdr's own `offset_from_bottom`, which is the daemon saying how far up a pane it
//! has actually moved.
//!
//! One test in this binary, on purpose - see `support`.

mod support;

use std::cell::Cell;

use herdr_harness::Daemon;
use muster::proto::{AttachPane, Attached, Response, Scroll, request, response};
use serde_json::{Value, json};
use support::{Bridge, Typing, answer, assert_ok, attach, named_pane, until};

/// Enough lines to push a 24-row pane well clear of its own scrollback floor.
const FILL: &str = "seq 1 200\n";

#[test]
fn a_wheel_moves_the_pane_it_names_rather_than_the_one_with_the_keyboard() {
    let typing = Typing::start("");
    let watched = typing.pane.clone();

    // A second pane beside the first, made through herdr's own API so that a broken routing
    // path fails at the assertion rather than at the arrangement.
    typing
        .daemon
        .call("pane.split", &json!({ "target_pane_id": watched.as_str(), "direction": "right" }));
    let typed_into = other_pane(&typing.daemon, &watched);

    // Attached in this order so the keyboard ends up on `typed_into`: attaching points it at
    // the pane attached, which is what makes the pair distinguishable at all.
    let attached = attach_once_the_core_knows_it(&named_pane(&typed_into));
    let _second =
        Bridge::spawn(&attached.backend_pane_id, &attached.control_socket_path, &typing.daemon);

    // Both panes get scrollback, so "the other one did not move" is a real assertion rather
    // than a pane that had nowhere to go.
    for pane in [&watched, &typed_into] {
        typing.daemon.call("pane.send_input", &json!({ "pane_id": pane, "text": FILL }));
        until(
            &format!("{pane} to have scrollback to move through"),
            || scroll_ceiling(&typing.daemon, pane) > 0,
            || format!("{pane} never filled its viewport, so a wheel there would do nothing"),
        );
    }

    assert_eq!(scroll_offset(&typing.daemon, &watched), 0, "the watched pane starts at the bottom");
    assert_eq!(scroll_offset(&typing.daemon, &typed_into), 0, "both panes start at the bottom");

    // The keyboard is on `typed_into`, and the wheel names `watched`. An empty daemon means
    // the one this window's keyboard is on, which is what every other message here means.
    // Named the way every message in this schema names a pane, while the oracle below reads
    // the daemon's own id: the routing this is about happens between the two.
    assert_ok(&answer(request::Payload::Scroll(Scroll {
        daemon_id: String::new(),
        pane_id: named_pane(&watched),
        direction: "up".to_string(),
        delta: 3.0,
    })));

    until(
        "the pane the wheel named to move up",
        || scroll_offset(&typing.daemon, &watched) > 0,
        || {
            format!(
                "{watched} is still at the bottom. If {typed_into} moved instead, the scroll \
                 was routed to the pane with the keyboard rather than to the pane it named - \
                 which is the whole of kan a_295OgKX9b.\n  {watched}: {}\n  {typed_into}: {}",
                scroll_offset(&typing.daemon, &watched),
                scroll_offset(&typing.daemon, &typed_into),
            )
        },
    );
    assert_eq!(
        scroll_offset(&typing.daemon, &typed_into),
        0,
        "the pane with the keyboard moved as well as the pane the wheel named, so the scroll \
         reached both"
    );
}

/// Attaches a pane the daemon has just made, once the core has heard about it.
///
/// The split is answered before the event describing it arrives, so an attach sent straight
/// after is refused for a pane that exists - a race in the arrangement rather than anything
/// this test is about.
fn attach_once_the_core_knows_it(pane: &str) -> Attached {
    let refusal = Cell::new(String::new());
    until(
        &format!("the core to hear about {pane}"),
        || match answer(request::Payload::AttachPane(AttachPane { pane_id: pane.to_string() })) {
            Response { payload: Some(response::Payload::Attached(_)) } => true,
            Response { payload: Some(response::Payload::Failure(failure)) } => {
                refusal.set(failure.reason);
                false
            }
            other => panic!("expected an attachment, got {other:?}"),
        },
        || format!("the core kept refusing: {}", refusal.take()),
    );
    attach(pane)
}

/// The pane in this session that is not the one named.
fn other_pane(daemon: &Daemon, known: &str) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    snapshot
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("panes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pane| pane.get("pane_id").and_then(Value::as_str))
        .find(|pane| *pane != known)
        .unwrap_or_else(|| panic!("the split should have given this tab a second pane: {snapshot}"))
        .to_string()
}

/// How far up its own history the daemon has this pane, by its own account.
fn scroll_offset(daemon: &Daemon, pane: &str) -> u64 {
    pane_scroll(daemon, pane, "offset_from_bottom")
}

/// How far up this pane could go, which is zero until something has scrolled off it.
fn scroll_ceiling(daemon: &Daemon, pane: &str) -> u64 {
    pane_scroll(daemon, pane, "max_offset_from_bottom")
}

fn pane_scroll(daemon: &Daemon, pane: &str, field: &str) -> u64 {
    let listed = daemon.call("pane.list", &json!({}));
    listed["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|held| held["pane_id"].as_str() == Some(pane))
        .and_then(|held| held["scroll"][field].as_u64())
        .unwrap_or_else(|| panic!("no {field} for {pane} in {listed}"))
}
