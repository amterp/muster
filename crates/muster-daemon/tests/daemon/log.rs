//! The daemon's own log (MIP-3 section 1): a bounded file beside its socket, and the same
//! records for any control connection that follows them.

use std::time::Duration;

use crate::support::*;
use muster_harness::Input;
use proto::input_event::{self, Input as Event};
use proto::session_request;

fn follow(control: &mut Control, after: Option<u64>) -> Asked {
    control.ask(session(session_request::Request::FollowLog(session_request::FollowLog { after })))
}

fn followed(asked: &Asked) -> proto::LogFollowed {
    match &asked.answer.detail {
        Some(proto::answer::Detail::Followed(followed)) => *followed,
        other => panic!("a follow answered with {other:?}"),
    }
}

const WAIT: Duration = Duration::from_secs(10);

/// A daemon outlives the run that started it, so the run's log is never its file, whatever the
/// environment it inherited says.
#[test]
fn the_daemon_keeps_its_own_log_beside_its_socket() {
    let run_log = std::env::temp_dir().join(format!("muster-run-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&run_log);
    let daemon = daemon_with(&[("MUSTER_LOG_FILE", run_log.to_str().unwrap())]);
    let own = daemon.root().join("daemon.log");
    until(
        "the daemon's own log to say it started",
        || written(&own).contains("daemon.started"),
        (),
    );
    assert!(!run_log.exists(), "the daemon wrote into the log of the run that started it");
}

#[test]
fn a_follower_gets_the_recent_log_then_every_new_record() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let asked = follow(&mut control, None);
    assert_eq!(asked.outcome(), proto::Outcome::Done);
    let span = followed(&asked);
    assert_eq!(span.oldest, 1, "a young daemon still holds its first record");

    assert!(
        control
            .logged_until("daemon.started", WAIT)
            .iter()
            .any(|l| l.line.contains("daemon.started"))
    );
    make(&mut control, create("p1", in_new_tab("t1")));
    let lines = control.logged_until("daemon.pane.started", WAIT).to_vec();
    assert!(lines.iter().any(|line| line.line.contains("daemon.pane.started")));
    let numbers: Vec<u64> = lines.iter().map(|line| line.number).collect();
    assert_eq!(numbers, (1..=numbers.len() as u64).collect::<Vec<_>>(), "numbered with no gaps");
    assert!(lines.iter().all(|line| line.line.starts_with('{') && !line.line.ends_with('\n')));

    let mut later = daemon.connect();
    let last = numbers.last().copied().unwrap();
    follow(&mut later, Some(last));
    make(&mut later, create("p2", in_new_tab("t2")));
    let resumed = later.logged_until("\"p2\"", WAIT);
    assert!(resumed.iter().all(|line| line.number > last), "nothing already had is sent again");
}

#[test]
fn what_a_pane_is_sent_never_reaches_the_log() {
    let daemon = daemon();
    let mut control = daemon.connect();
    let mut input = Input::connect(daemon.socket_path());
    follow(&mut control, None);
    make(&mut control, create("p1", in_new_tab("t1")));
    let secret = "hunter2-correct-horse";
    input
        .send("p1", Event::Send(input_event::Send { text: format!("echo {secret}"), enter: true }));
    until_text(&mut control, "p1", &format!("{secret}\n"));
    make(&mut control, create("p2", in_new_tab("t2")));

    let followed = control.logged_until("\"p2\"", WAIT);
    assert!(followed.iter().any(|line| line.line.contains("\"p2\"")));
    assert!(!followed.iter().any(|line| line.line.contains(secret)), "the followed log has it");
    assert!(!written(&daemon.root().join("daemon.log")).contains(secret), "the file has it");
}

#[test]
fn with_logging_off_there_is_no_file_and_nothing_to_follow() {
    let daemon = daemon_with(&[("MUSTER_LOG", "0")]);
    let mut control = daemon.connect();
    let refused = follow(&mut control, None);
    assert_eq!(refused.outcome(), proto::Outcome::Refused);
    assert!(refused.answer.reason.contains("MUSTER_LOG=0"), "{}", refused.answer.reason);
    assert!(!daemon.root().join("daemon.log").exists());
}
