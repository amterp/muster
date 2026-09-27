//! A lost answer, staged in front of a real daemon.

use std::time::{Duration, Instant};

use crate::support::*;
use muster_harness::Control;
use proto::request::Service;

fn is_create(request: &proto::Request) -> bool {
    matches!(
        &request.service,
        Some(Service::Pane(proto::PaneRequest {
            request: Some(proto::pane_request::Request::Create(_))
        }))
    )
}

#[test]
fn a_withheld_answer_never_arrives_while_the_work_and_its_events_do() {
    let daemon = daemon();
    let relay = daemon.withholding_answers_where(is_create);
    let mut relayed = Control::connect(relay.socket_path());
    expect(&mut relayed, subscribe_request(), proto::Outcome::Done);

    relayed.send(create_request(create("p1", in_new_tab("t1"))));
    assert_eq!(
        names(&[relayed.next_event(), relayed.next_event()]),
        ["pane_opened:p1", "tab_opened:t1"]
    );
    // Events are written before their answer, and the snapshot below is answered after both, so
    // an answer that was coming would already be here.
    let mut direct = daemon.connect();
    assert_eq!(snapshot(&mut direct).panes.len(), 1);
    assert!(
        relayed.next_message(Duration::from_millis(50)).is_none(),
        "the withheld answer arrived"
    );

    // Everything else still passes.
    snapshot(&mut relayed);
}

#[test]
fn a_delayed_answer_arrives_late() {
    let daemon = daemon();
    let delay = Duration::from_millis(300);
    let relay = daemon.delaying_answers_where(is_create, delay);
    let mut relayed = Control::connect(relay.socket_path());
    let asked_at = Instant::now();
    make(&mut relayed, create("p1", in_new_tab("t1")));
    assert!(asked_at.elapsed() >= delay, "answered after {:?}", asked_at.elapsed());
}
