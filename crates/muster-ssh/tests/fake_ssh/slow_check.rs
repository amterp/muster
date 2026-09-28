//! A check that does not answer in time, and what the supervisor makes of it.

use std::time::Duration;

use crate::fake;

#[test]
fn one_slow_check_leaves_a_working_master_alone() {
    // What ended seven panes' bridges on a working connection (kan a_2YQCqiInL). A check
    // that times out is a master that is slow or wedged, never a dropped network: a master
    // whose connection dies exits once ServerAlive gives up, and the next check then fails at
    // once. So one timeout is asked again before anything is ended.
    let scratch = fake::Scratch::new();
    let forward = fake::forward(scratch.path());
    let control = forward.control_path.clone();
    fake::slow_checks(&control, 1);
    let tunnel = fake::open(forward);

    fake::until("two checks after the slow one", &control, Duration::from_secs(15), || {
        fake::count(&control, "control check") >= 3
    });
    assert_eq!(
        fake::count(&control, "control exit"),
        0,
        "one slow check ended a master that answered the next one, which drops every pane's \
         bridge on that machine for nothing: {:?}",
        fake::asked(&control)
    );
    assert_eq!(fake::count(&control, "master"), 1, "a second master was started");
    drop(tunnel);
}

#[test]
fn a_master_that_stays_silent_is_ended_and_reopened() {
    // The other half: patience is one poll, not forever. A wedged master that would not answer
    // twice running is ended and replaced, as any master that stops answering is.
    let scratch = fake::Scratch::new();
    let forward = fake::forward(scratch.path());
    let control = forward.control_path.clone();
    fake::slow_checks(&control, 2);
    let tunnel = fake::open(forward);

    fake::until("a second master", &control, Duration::from_secs(20), || {
        fake::count(&control, "master") >= 2
    });
    assert!(
        fake::count(&control, "control exit") >= 1,
        "a new master was started without the silent one being asked to leave: {:?}",
        fake::asked(&control)
    );
    drop(tunnel);
}
