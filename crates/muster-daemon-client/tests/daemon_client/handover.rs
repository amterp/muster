//! An older daemon hands its panes to this build's (MIP-3, section 10).
//!
//! Today's daemon is the only one a test can build, so the older one is it saying an older
//! version (`MUSTER_DAEMON_VERSION_SAID`, read only by a debug build).

use std::path::Path;

use muster_daemon_client::handover::{self, Age, Handed, NotHanded};
use muster_daemon_proto::Welcome;
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{DAEMON_DATA, Daemon, built_daemon};

#[test]
fn an_older_daemon_hands_its_panes_over_and_they_carry_on() {
    let mut daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let before = daemon.connect().welcome().clone();
    assert_eq!(handover::age(&before.daemon_version), Age::Older);

    let after = handed(ask(&daemon, before.instance));
    daemon.served_by(after.pid.cast_signed());

    assert_ne!(after.instance, before.instance, "the same daemon is still serving");
    let panes: Vec<String> =
        snapshot(&mut daemon.connect()).panes.into_iter().map(|pane| pane.pane).collect();
    assert_eq!(panes, ["p1"], "the pane did not come through the handoff");
}

#[test]
fn a_refused_handoff_leaves_the_daemon_serving_and_says_why() {
    let daemon = Daemon::start_with(
        built_daemon(),
        &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1"), ("MUSTER_DAEMON_HANDOFF_FAULT", "refuse")],
    );
    let before = daemon.connect().welcome().clone();

    let refused = match ask(&daemon, before.instance) {
        Err(NotHanded::Kept(reason)) => reason,
        other => panic!("a handoff the successor refused was reported as {other:?}"),
    };

    assert!(!refused.is_empty(), "the refusal says nothing");
    assert_eq!(
        daemon.connect().welcome().instance,
        before.instance,
        "a refused handoff left another daemon serving"
    );
}

#[test]
fn a_window_that_found_the_old_daemon_does_not_ask_the_new_one() {
    // Two windows adopt the same older daemon, and the first hands it over before the second
    // asks. The second used to ask whoever answered, so the new daemon handed every pane to a
    // copy of itself: a second handoff, on the one path that can end every agent at once.
    let mut daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let found = daemon.connect().welcome().instance;
    let first = handed(ask(&daemon, found));
    daemon.served_by(first.pid.cast_signed());

    let second = ask(&daemon, found);

    let serving = daemon.connect().welcome().instance;
    assert_eq!(serving, first.instance, "the second window handed the new daemon over again");
    assert!(
        matches!(&second, Ok(Handed::ByAnother(welcome)) if welcome.instance == first.instance),
        "the second window reported {second:?} for a handoff somebody else made"
    );
}

#[test]
fn a_window_asking_mid_handoff_does_not_ask_the_new_daemon() {
    // A daemon handing over stops taking connections, and one made meanwhile waits on the
    // socket and is taken by the new daemon, which the socket goes to. The window that made it
    // found the old daemon, and asked whoever answered.
    let mut daemon = Daemon::start_with(
        built_daemon(),
        &[
            ("MUSTER_DAEMON_VERSION_SAID", "0.0.1"),
            ("MUSTER_DAEMON_HANDOFF_FAULT", "pause-before-hold"),
        ],
    );
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let found = daemon.connect().welcome().instance;
    let first = asking(&daemon, found);
    daemon.paused();
    let second = asking(&daemon, found);
    std::thread::sleep(std::time::Duration::from_secs(1));
    daemon.resume();

    let done = handed(first.join().expect("the first window's thread panicked"));
    daemon.served_by(done.pid.cast_signed());
    let second = second.join().expect("the second window's thread panicked");
    assert!(
        matches!(&second, Ok(Handed::ByAnother(welcome)) if welcome.instance == done.instance),
        "the second window reported {second:?} for a handoff somebody else made"
    );
    assert_eq!(daemon.connect().welcome().instance, done.instance, "it was handed over twice");
}

#[test]
fn two_windows_asking_at_once_hand_over_once_and_neither_is_refused() {
    // Both are taken while the first is still running the new daemon once to see that it
    // starts, before anything is marked; whichever gets there second is refused as "already
    // being replaced". That is somebody else's handoff going through, and a window that
    // reported it as a refusal would warn about a handoff that succeeded.
    let mut daemon = Daemon::start_with(
        built_daemon(),
        &[
            ("MUSTER_DAEMON_VERSION_SAID", "0.0.1"),
            ("MUSTER_DAEMON_HANDOFF_FAULT", "pause-after-launch"),
        ],
    );
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let found = daemon.connect().welcome().instance;
    let first = asking(&daemon, found);
    daemon.paused();
    let second = asking(&daemon, found);
    // Long enough for the second request to reach the same pause, which takes milliseconds.
    std::thread::sleep(std::time::Duration::from_secs(1));
    daemon.resume();

    let first = first.join().expect("the first window's thread panicked");
    let second = second.join().expect("the second window's thread panicked");
    // Which of the two did it is not always knowable: a window whose connection the old daemon
    // ended as it handed over cannot tell its own request from the other's. That they agree on
    // one new daemon, and that neither reports a refusal, is.
    let serving = |answer: &Result<Handed, NotHanded>| match answer {
        Ok(Handed::Over(welcome) | Handed::ByAnother(welcome)) => Some(welcome.clone()),
        Err(_) => None,
    };
    let (Some(one), Some(other)) = (serving(&first), serving(&second)) else {
        panic!("a window reported a refusal while a handoff went through: {first:?}, {second:?}");
    };
    daemon.served_by(one.pid.cast_signed());
    assert_eq!(one.instance, other.instance, "the two windows handed over twice");
    assert_eq!(daemon.connect().welcome().instance, one.instance, "it was handed over again");
}

/// A window asking, on a thread of its own, about the daemon it found.
fn asking(daemon: &Daemon, found: u64) -> std::thread::JoinHandle<Result<Handed, NotHanded>> {
    let socket = daemon.socket_path().to_path_buf();
    std::thread::spawn(move || ask_on(&socket, found))
}

fn ask(daemon: &Daemon, found: u64) -> Result<Handed, NotHanded> {
    ask_on(daemon.socket_path(), found)
}

fn ask_on(socket: &Path, found: u64) -> Result<Handed, NotHanded> {
    handover::hand_over(socket, &built_daemon(), Some(Path::new(DAEMON_DATA)), found)
}

fn handed(asked: Result<Handed, NotHanded>) -> Welcome {
    match asked {
        Ok(Handed::Over(welcome)) => welcome,
        other => panic!("the older daemon did not hand its panes over: {other:?}"),
    }
}
