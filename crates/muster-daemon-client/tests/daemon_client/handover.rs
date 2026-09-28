//! An older daemon hands its panes to this build's (MIP-3, section 10).
//!
//! Today's daemon is the only one a test can build, so the older one is it saying an older
//! version (`MUSTER_DAEMON_VERSION_SAID`, read only by a debug build).

use std::path::Path;

use muster_daemon_client::handover::{self, Age};
use muster_harness::requests::{create, in_new_tab, make, snapshot};
use muster_harness::{DAEMON_DATA, Daemon, built_daemon};

#[test]
fn an_older_daemon_hands_its_panes_over_and_they_carry_on() {
    let mut daemon = Daemon::start_with(built_daemon(), &[("MUSTER_DAEMON_VERSION_SAID", "0.0.1")]);
    make(&mut daemon.connect(), create("p1", in_new_tab("t1")));
    let before = daemon.connect().welcome().clone();
    assert_eq!(handover::age(&before.daemon_version), Age::Older);

    let after =
        handover::hand_over(daemon.socket_path(), &built_daemon(), Some(Path::new(DAEMON_DATA)))
            .expect("the older daemon handed its panes over");
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

    let refused =
        handover::hand_over(daemon.socket_path(), &built_daemon(), Some(Path::new(DAEMON_DATA)))
            .expect_err("a handoff the successor refused was reported as done");

    assert!(!refused.is_empty(), "the refusal says nothing");
    assert_eq!(
        daemon.connect().welcome().instance,
        before.instance,
        "a refused handoff left another daemon serving"
    );
}
