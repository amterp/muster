//! What a pane's control socket does across the life of the pane.
//!
//! Two properties, and both are about a pane whose surface was thrown away and built again -
//! which happens whenever a window has to rebuild a pane, and used to end with that pane
//! rendering, painting, and swallowing every keystroke.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use muster_herdr::PaneControlChannel;
use muster_herdr::control_stream::ControlStreamMessage;

#[test]
fn a_replacement_bridge_can_still_connect() {
    // A pane keeps its channel while its surface is rebuilt, so the socket has to be there
    // for the bridge that replaces the one that went. A listener that accepted once left
    // that pane unable to be typed into for the rest of the session, and said nothing.
    let path = socket_path("replacement");
    let connections = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&connections);
    let channel = PaneControlChannel::bind(path.clone(), move || {
        counting.fetch_add(1, Ordering::Release);
    })
    .expect("the socket binds");

    let first = UnixStream::connect(&path).expect("the first bridge connects");
    until("the first bridge is noticed", || connections.load(Ordering::Acquire) == 1);
    drop(first);

    let mut second = UnixStream::connect(&path).expect("a replacement bridge connects");
    until("the replacement is noticed", || connections.load(Ordering::Acquire) == 2);

    // And it is the one that gets the input, not the one it replaced.
    assert!(channel.send(&ControlStreamMessage::Input(b"hello".to_vec())));
    let mut arrived = [0u8; 8];
    second.read_exact(&mut arrived).expect("the replacement receives what was sent");
    assert!(!arrived.iter().all(|byte| *byte == 0));
}

#[test]
fn closing_a_pane_does_not_leave_a_thread_behind() {
    // The thread is parked inside `accept` and nothing else will ever wake it. One per pane
    // that closed, for as long as the window is open - invisible from outside the process,
    // and unobservable through any other API, which is why the flag exists.
    let channel =
        PaneControlChannel::bind(socket_path("stopping"), || {}).expect("the socket binds");
    let stopped = channel.stopped();
    assert!(!stopped.load(Ordering::Acquire), "it is still accepting while the pane is open");

    drop(channel);

    until("the accepting thread stops", || stopped.load(Ordering::Acquire));
}

fn socket_path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("muster-test-{}-{name}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

/// Waits for something another thread does, or fails saying what never happened.
fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{what} did not happen within two seconds");
}
