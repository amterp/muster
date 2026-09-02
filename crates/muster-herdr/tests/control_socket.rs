//! What a pane's control socket does across the life of the pane.
//!
//! Three properties. Two are about a pane whose surface was thrown away and built again -
//! which happens whenever a window has to rebuild a pane, and used to end with that pane
//! rendering, painting, and swallowing every keystroke. The third is the other end of the
//! same connection: it is how the app finds out a bridge has died, which until kan
//! a_2IRcMjFs0 nothing in the app ever did.

use herdr_harness::until;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use muster_core::respawn::Ending;
use muster_herdr::bridge_report::Exiting;
use muster_herdr::control_stream::ControlStreamMessage;
use muster_herdr::{Ended, PaneControlChannel, Reports};

#[test]
fn a_replacement_bridge_can_still_connect() {
    // A pane keeps its channel while its surface is rebuilt, so the socket has to be there
    // for the bridge that replaces the one that went. A listener that accepted once left
    // that pane unable to be typed into for the rest of the session, and said nothing.
    let path = socket_path("replacement");
    let watch = Watch::default();
    let channel =
        PaneControlChannel::bind(path.clone(), watch.reports()).expect("the socket binds");

    let first = UnixStream::connect(&path).expect("the first bridge connects");
    until("the first bridge is noticed", || watch.connections() == 1, ());
    drop(first);

    let mut second = UnixStream::connect(&path).expect("a replacement bridge connects");
    until("the replacement is noticed", || watch.connections() == 2, ());

    // And it is the one that gets the input, not the one it replaced.
    assert!(channel.send(&ControlStreamMessage::Input(b"hello".to_vec())));
    let mut arrived = [0u8; 8];
    second.read_exact(&mut arrived).expect("the replacement receives what was sent");
    assert!(!arrived.iter().all(|byte| *byte == 0));
}

#[test]
fn a_bridge_that_stops_is_reported_with_what_it_said() {
    // The keystone of kan a_2IRcMjFs0. Every bridge that has ever died in the field died
    // without the app noticing, because the only thing watching was libghostty's
    // close_surface and a dead pane never triggers it. This connection is the app's own, and
    // it ends when the process does.
    let path = socket_path("exit");
    let watch = Watch::default();
    let _channel =
        PaneControlChannel::bind(path.clone(), watch.reports()).expect("the socket binds");

    let mut bridge = UnixStream::connect(&path).expect("the bridge connects");
    until("the bridge is noticed", || watch.connections() == 1, ());

    let refusal = "terminal term_1 already has an attached client; retry with --takeover";
    let said =
        Exiting { ending: Ending::Refused, reason: Some(refusal.to_string()), rendered: false };
    bridge.write_all(&said.wire_format()).expect("the bridge reports why it is stopping");
    bridge.flush().expect("and it arrives");
    drop(bridge);

    until(
        "the app to be told the bridge is gone",
        || watch.ended().is_some(),
        || {
            "nothing reported the exit, so the respawn policy is never reached and the pane \
         stays dark until somebody relaunches"
                .to_string()
        },
    );
    let ended = watch.ended().expect("just waited for it");
    // The ending, because that is what decides whether another bridge is worth starting, and
    // the reason, because that is the only sentence anybody can act on - it names the terminal
    // and the machine, and Muster cannot compose either.
    assert_eq!(ended.ending, Ending::Refused);
    assert_eq!(ended.reason.as_deref(), Some(refusal));
    assert!(!ended.rendered);
}

#[test]
fn a_bridge_that_was_replaced_is_not_reported_as_having_died() {
    // The replacement's own arrival is what ends the one it replaced, so reporting that would
    // have the core count a replacement against a pane it had just given one to - and three of
    // those inside thirty seconds is a pane Muster gives up on.
    let path = socket_path("replaced");
    let watch = Watch::default();
    let _channel =
        PaneControlChannel::bind(path.clone(), watch.reports()).expect("the socket binds");

    let first = UnixStream::connect(&path).expect("the first bridge connects");
    until("the first bridge is noticed", || watch.connections() == 1, ());
    let second = UnixStream::connect(&path).expect("the replacement connects");
    until("the replacement is noticed", || watch.connections() == 2, ());

    // The channel shut the first connection down when the second arrived, so its reader has
    // already woken and decided. Waiting on the live one proves the quiet is a decision rather
    // than a race this test won.
    drop(first);
    drop(second);
    until("the live bridge going to be reported", || watch.ended().is_some(), ());
    assert_eq!(watch.exits(), 1, "the bridge that was replaced was reported as a second death");
}

#[test]
fn closing_a_pane_does_not_leave_a_thread_behind() {
    // The thread is parked inside `accept` and nothing else will ever wake it. One per pane
    // that closed, for as long as the window is open - invisible from outside the process,
    // and unobservable through any other API, which is why the flag exists.
    let watch = Watch::default();
    let channel =
        PaneControlChannel::bind(socket_path("stopping"), watch.reports()).expect("it binds");
    let stopped = channel.stopped();
    assert!(!stopped.load(Ordering::Acquire), "it is still accepting while the pane is open");

    drop(channel);

    until("the accepting thread stops", || stopped.load(Ordering::Acquire), ());
}

/// What a channel told its owner, for a test to read back.
#[derive(Default)]
struct Watch {
    connections: Arc<AtomicUsize>,
    exits: Arc<Mutex<Vec<Ended>>>,
}

impl Watch {
    fn reports(&self) -> Reports {
        let connections = Arc::clone(&self.connections);
        let exits = Arc::clone(&self.exits);
        Reports {
            connected: Box::new(move || {
                connections.fetch_add(1, Ordering::Release);
            }),
            exited: Box::new(move |ended| {
                exits.lock().expect("a panicking test poisoned the exits").push(ended);
            }),
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    fn exits(&self) -> usize {
        self.exits.lock().expect("a panicking test poisoned the exits").len()
    }

    fn ended(&self) -> Option<Ended> {
        self.exits.lock().expect("a panicking test poisoned the exits").first().cloned()
    }
}

fn socket_path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("muster-test-{}-{name}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}
