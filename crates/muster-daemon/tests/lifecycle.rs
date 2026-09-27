//! Starting, stopping, and being the one daemon on a socket.

mod support;

use muster_harness::FIRST_ANSWER_BUDGET;
use proto::session_request;
use support::*;

/// A daemon-backed test is only in the default gate while a daemon costs about as much as
/// herdr's did to start (`docs/testing.md`), so this holds muster-daemon to that.
///
/// The best of several starts, not each one: macOS assesses a freshly built binary the first
/// time it runs (about a second), and the gate runs other daemons' tests beside this one.
/// Twenty starts of a debug build measured 3.9 ms at the fastest and 4.1 ms at the median, on a
/// machine at a load average of 30, so the fastest of five fails only when every start has got
/// several times slower.
#[test]
fn a_spawned_daemon_answers_its_first_request_within_budget() {
    let starts: Vec<_> = (0..5).map(|_| daemon().started_in()).collect();
    let fastest = starts.iter().min().unwrap();
    assert!(
        *fastest <= FIRST_ANSWER_BUDGET,
        "the fastest of five daemons took {fastest:?} to answer (all: {starts:?}), over the \
         {FIRST_ANSWER_BUDGET:?} budget.\n  Impact: daemon-backed tests cost more than the \
         default gate can carry.\n  Check what the daemon does before it binds its socket."
    );
}

#[test]
fn a_second_daemon_on_a_claimed_socket_leaves_the_first_serving() {
    let daemon = daemon();
    let mut second = daemon.spawn_another();
    let status = second.wait().unwrap();
    assert_eq!(status.code(), Some(3), "a second daemon exits 3, meaning another is serving");
    daemon.connect();
}

#[test]
fn stop_closes_every_pane_answers_and_exits() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    expect(&mut control, subscribe_request(), proto::Outcome::Done);
    make(&mut control, create("p1", in_new_tab("t1")));
    make(&mut control, create("p2", beside("p1", proto::Side::Right)));

    let asked = expect(
        &mut control,
        session(session_request::Request::Stop(session_request::Stop {})),
        proto::Outcome::Done,
    );
    assert_eq!(names(&asked.events), ["tab_closed:t1", "pane_closed:p1", "pane_closed:p2"]);
    assert!(daemon.wait_for_exit().success());
    assert!(!daemon.socket_path().exists(), "the daemon removes its socket on the way out");
}

#[test]
fn sigterm_stops_the_daemon_the_same_way() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    make(&mut control, create("p1", in_new_tab("t1")));
    // SAFETY: kill signals the harness's own child.
    unsafe {
        libc::kill(daemon.pid().cast_signed(), libc::SIGTERM);
    }
    assert!(daemon.wait_for_exit().success());
    assert!(!daemon.socket_path().exists());
}

#[test]
fn sighup_does_not_stop_the_daemon() {
    let daemon = daemon();
    // SAFETY: kill signals the harness's own child.
    unsafe {
        libc::kill(daemon.pid().cast_signed(), libc::SIGHUP);
    }
    let mut control = daemon.connect();
    snapshot(&mut control);
}
