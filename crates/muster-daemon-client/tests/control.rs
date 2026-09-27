//! The control connection the app will follow a daemon on, against the real daemon.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use muster_daemon_client::control::{Control, Delivered, Unanswered};
use muster_daemon_proto as proto;
use muster_harness::requests::*;
use muster_harness::{Daemon, PATIENCE};

fn open(daemon: &Daemon) -> (Control, Receiver<Delivered>) {
    let (tell, delivered) = channel();
    let control = Control::open(daemon.socket_path(), "test", move |what| {
        let _ = tell.send(what);
    })
    .expect("the daemon welcomes a control connection");
    (control, delivered)
}

fn answered(pending: &muster_daemon_client::control::Pending) -> proto::Answer {
    pending.wait(PATIENCE).expect("the daemon answers")
}

fn next_event(delivered: &Receiver<Delivered>) -> proto::Event {
    match delivered.recv_timeout(PATIENCE) {
        Ok(Delivered::Event(event)) => *event,
        other => panic!("expected an event, got {other:?}"),
    }
}

#[test]
fn a_subscription_starts_at_its_snapshot_and_a_request_is_answered_after_its_events() {
    let daemon = Daemon::start_built();
    let (control, delivered) = open(&daemon);
    let subscribed = answered(&control.subscribe());
    let Some(proto::answer::Detail::Snapshot(snapshot)) = subscribed.detail else {
        panic!("a subscribe answers with a snapshot: {subscribed:?}");
    };
    assert!(snapshot.panes.is_empty());

    let created = answered(&control.ask(create_request(create("p1", in_new_tab("t1")))));
    assert_eq!(created.outcome(), proto::Outcome::Done, "{}", created.reason);

    let mut events = vec![next_event(&delivered)];
    while events.last().unwrap().seq < created.seq {
        events.push(next_event(&delivered));
    }
    assert_eq!(events[0].seq, snapshot.seq + 1, "the first event follows the snapshot");
    assert_eq!(
        events.last().unwrap().seq,
        created.seq,
        "every event the create produced was delivered before its answer, which names the last"
    );
    assert!(names(&events).contains(&"pane_opened:p1".to_string()), "{:?}", names(&events));
}

#[test]
fn outcomes_say_what_happened() {
    let daemon = Daemon::start_built();
    let (control, _delivered) = open(&daemon);
    assert_eq!(
        answered(&control.ask(close_request("nowhere"))).outcome(),
        proto::Outcome::NotThere
    );

    let bar = proto::Cursor { style: proto::CursorStyle::Bar.into(), blink: Some(false) };
    assert_eq!(answered(&control.set_cursor(bar)).outcome(), proto::Outcome::Done);
    assert_eq!(answered(&control.set_cursor(bar)).outcome(), proto::Outcome::AlreadySo);
}

#[test]
fn a_stopped_daemon_ends_the_connection_and_exits() {
    let mut daemon = Daemon::start_built();
    let (control, delivered) = open(&daemon);
    assert_eq!(answered(&control.stop()).outcome(), proto::Outcome::Done);
    assert!(
        delivered.iter().any(|what| matches!(what, Delivered::Ended(_))),
        "the end is delivered"
    );
    assert!(daemon.wait_for_exit().success());
}

/// Whatever is asked once the daemon has died is answered at once, rather than left to time out.
#[test]
fn a_request_to_a_daemon_that_died_is_answered_as_ended() {
    let mut daemon = Daemon::start_built();
    let (control, _delivered) = open(&daemon);
    daemon.kill();
    let pending = control.snapshot();
    assert_eq!(pending.wait(Duration::from_secs(5)).unwrap_err(), Unanswered::Ended);
}
