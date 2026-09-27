//! Starting, stopping, and being the one daemon on a socket.

use crate::support::*;
use muster_harness::FIRST_ANSWER_BUDGET;
use proto::session_request;

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

#[test]
fn a_socket_path_naming_a_file_is_refused_rather_than_deleted() {
    let daemon = daemon();
    let mistyped = daemon.root().join("notes.txt");
    std::fs::write(&mistyped, "somebody's notes\n").unwrap();
    let mut refused = std::process::Command::new(env!("CARGO_BIN_EXE_muster-daemon"))
        .arg("--socket")
        .arg(&mistyped)
        .arg("--data")
        .arg(DAEMON_DATA)
        .env_clear()
        .env("HOME", daemon.root().join("home"))
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut status = None;
    let exited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        until(
            "a daemon given a regular file as its socket to exit",
            || {
                status = refused.try_wait().unwrap();
                status.is_some()
            },
            (),
        );
    }));
    if exited.is_err() {
        let _ = refused.kill();
        let _ = refused.wait();
    }
    assert_eq!(
        std::fs::read_to_string(&mistyped).ok().as_deref(),
        Some("somebody's notes\n"),
        "the file at the socket path was replaced"
    );
    assert!(
        !status.expect("the daemon exited").success(),
        "a daemon took a regular file as its socket"
    );
    let mut said = String::new();
    std::io::Read::read_to_string(&mut refused.stderr.take().unwrap(), &mut said).unwrap();
    assert!(said.contains("not a socket"), "{said}");
}

/// A pane whose program ignores the hang-up, and its pid once the program has said it.
fn deaf_pane(control: &mut Control) -> String {
    let command = "echo pid=$$.; trap '' HUP; while :; do sleep 1; done";
    let create = proto::pane_request::Create {
        command: Some(command.to_string()),
        ..create("p1", in_new_tab("t1"))
    };
    make(control, create);
    let text = until_text(control, "p1", "pid=");
    let at = text.rfind("pid=").unwrap() + 4;
    text[at..].split('.').next().unwrap().to_string()
}

/// Past this after the hang-up, a program still running has been killed.
const KILLED_WITHIN: std::time::Duration = std::time::Duration::from_secs(6);

fn gone_within(pid: &str, within: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if process_state(pid).is_empty() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // So a failed run leaves nothing behind.
    let _ = std::process::Command::new("kill").args(["-9", pid]).status();
    false
}

/// A program that ignores SIGHUP would otherwise outlive the pane that ran it, holding whatever
/// it held, for as long as the machine runs.
#[test]
fn a_program_that_ignores_the_hang_up_is_killed_when_its_pane_closes() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let pid = deaf_pane(&mut control);
    expect(&mut control, close_request("p1"), proto::Outcome::Done);
    assert!(gone_within(&pid, KILLED_WITHIN), "pid {pid} outlived its pane");
}

/// The daemon stays until what it hung up is gone or killed, since nothing would kill it after.
#[test]
fn a_daemon_that_stops_kills_what_ignores_the_hang_up_before_it_exits() {
    let mut daemon = daemon();
    let mut control = daemon.connect();
    let pid = deaf_pane(&mut control);
    expect(
        &mut control,
        session(session_request::Request::Stop(session_request::Stop {})),
        proto::Outcome::Done,
    );
    daemon.wait_for_exit();
    assert!(
        gone_within(&pid, std::time::Duration::from_millis(500)),
        "pid {pid} outlived the daemon"
    );
}
