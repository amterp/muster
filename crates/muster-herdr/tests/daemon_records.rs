//! Whether the census finds a real daemon, and whether it notices when one goes.
//!
//! The record is a hint that gets checked, and the check is the whole feature: a census built
//! from files alone would report a daemon that ended half an hour ago as a running process,
//! which is the opposite of useful to somebody about to end one. So every assertion here is
//! about the dial rather than about the file.
//!
//! A real daemon, because what is being tested is what dialing a socket says. A stand-in would
//! be Muster's own guess at a daemon and a wrong guess passes.

use std::path::PathBuf;

use herdr_harness::Daemon;
use muster_herdr::records::{self, State};

#[test]
fn a_daemon_that_is_running_is_answering_and_says_what_it_holds() {
    let directory = scratch("records-answering");
    let daemon = Daemon::start();
    let socket = daemon.socket_path().to_string_lossy().into_owned();
    daemon.call(
        "workspace.create",
        &serde_json::json!({ "cwd": "/tmp", "label": "census", "focus": true }),
    );

    records::started(&directory, &socket);

    let found = records::census(&directory);
    let [only] = found.as_slice() else {
        panic!("one daemon was written down and the census answered with {found:?}");
    };
    assert_eq!(only.socket, socket);
    assert_eq!(only.state, State::Answering);
    assert!(
        only.started > 0,
        "a record with no time in it cannot be recognised by whoever started the daemon"
    );
    assert_eq!(
        only.panes, 1,
        "the census has to say what a daemon holds, not just that it is there. A count is the \
         whole decision: of twenty daemons on one machine, nineteen held nothing and one held \
         somebody's live agent."
    );
    assert_eq!(only.directories, vec!["/private/tmp".to_string()]);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_daemon_that_has_ended_is_not_reported_as_running() {
    let directory = scratch("records-ended");
    let mut daemon = Daemon::start();
    let socket = daemon.socket_path().to_string_lossy().into_owned();
    records::started(&directory, &socket);
    assert_eq!(
        records::census(&directory).first().map(|found| found.state),
        Some(State::Answering)
    );

    // Killed rather than asked to stop, which is the case that matters: a daemon asked to stop
    // takes its socket file with it, and a daemon that was shot leaves the file behind. Those
    // are the two states the census has to tell apart, and the second is the one a file-only
    // answer would report as a running process.
    daemon.kill();

    let found = records::census(&directory);
    let [only] = found.as_slice() else {
        panic!("the record outlives the daemon, and the census answered with {found:?}");
    };
    assert_ne!(
        only.state,
        State::Answering,
        "a daemon that has been killed is still in the record and must not be reported as \
         answering.\n  Impact: somebody deciding what to end is shown a process that is not \
         there, and the row beside it - the one holding their agents - reads the same way.\n  \
         Check that `records::look` dials the socket rather than trusting the file."
    );
    assert_eq!(only.socket, socket, "the row is still about the daemon that was written down");
    assert_eq!(only.panes, 0, "a daemon that did not answer was not asked what it holds");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_daemon_restarted_on_one_socket_is_one_row_rather_than_two() {
    let directory = scratch("records-restarted");
    let daemon = Daemon::start();
    let socket = daemon.socket_path().to_string_lossy().into_owned();

    records::started(&directory, &socket);
    records::started(&directory, &socket);
    records::started(&directory, &socket);

    assert_eq!(
        records::census(&directory).len(),
        1,
        "the socket is the identity, so a daemon written down three times is one daemon. A \
         census that grew a row per restart would be the unreadable thing it exists to fix."
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_directory_nothing_has_written_answers_with_nothing() {
    // Not an error and not a refusal: a machine where Muster has started no daemon is an
    // ordinary machine, and the answer to what is on it is nothing.
    let directory = scratch("records-empty");

    assert!(records::census(&directory).is_empty());
    assert!(
        records::census("/tmp/muster-test/a-directory-that-does-not-exist").is_empty(),
        "a census of somewhere that is not there answers with nothing rather than failing - the \
         window works either way, and this is a read"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

fn scratch(name: &str) -> String {
    let path = PathBuf::from(format!("/tmp/muster-test/{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path.to_string_lossy().into_owned()
}
