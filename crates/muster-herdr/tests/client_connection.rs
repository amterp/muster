//! How Muster holds a connection open while it asks, which is not a detail.
//!
//! Every call goes down a connection of its own and the client used to half-close the write
//! side after sending, on the belief that herdr waits for end-of-write before answering. It
//! does not - it reads one request line - and for one call the difference is total: given a
//! half-closed socket, `pane.wait_for_output` reads it as a caller that has gone and hangs up
//! without answering. What that looked like from Muster was a timeout in under a millisecond.
//!
//! Pinned here because nothing else would notice it coming back. The ordinary calls answer
//! either way, so a half-close reintroduced for tidiness would pass every other test in the
//! suite and break only the one thing that has to wait.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use herdr_harness::Daemon;
use muster_herdr::HerdrClient;
use muster_herdr::client::Failure;
use serde_json::{Value, json};

#[test]
fn a_call_the_daemon_answers_slowly_is_answered_at_all() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "waiting", "focus": true }));
    let pane = only_pane(&daemon);
    let client = HerdrClient::new(daemon.socket_path().to_string_lossy().into_owned());

    // Any non-space on the visible screen, which is what "the shell has drawn its prompt"
    // amounts to. Given longer than the client's own default, because the point of this call is
    // to wait and the default is sized for a keystroke.
    let waiting = json!({
        "pane_id": pane,
        "match": { "type": "regex", "value": "\\S" },
        "source": "visible",
        "timeout_ms": 5_000,
    });
    let answer = client
        .request_within("pane.wait_for_output", &waiting, Duration::from_secs(10))
        .expect("a shell that has drawn a prompt has non-space on its screen");
    assert_eq!(
        answer["type"].as_str(),
        Some("output_matched"),
        "the wait answered with something else: {answer}"
    );

    // And the same call over a half-closed socket, which is the arrangement this exists to rule
    // out. Raw rather than through the client, because the client no longer has a way to do it -
    // this is the assertion that says why it must not grow one back.
    assert_eq!(
        raw_call(daemon.socket_path(), "pane.wait_for_output", &waiting, true),
        None,
        "herdr answered a half-closed caller after all, so the client's shape is no longer \
         load-bearing and this test is the thing that is now wrong"
    );

    // Ordinary calls answer either way, which is why nothing else in the suite guards this.
    for method in ["ping", "session.snapshot"] {
        for half_closed in [true, false] {
            assert!(
                raw_call(daemon.socket_path(), method, &json!({}), half_closed).is_some(),
                "{method} went unanswered with the write side half_closed={half_closed}"
            );
        }
    }
}

/// A timeout Muster asked for comes back as a refusal it can read.
///
/// The other half of what makes the wait usable: a shell configured with an empty prompt draws
/// nothing, and Muster has to be able to tell that apart from a daemon that has gone.
#[test]
fn a_wait_that_finds_nothing_says_it_timed_out() {
    let daemon = Daemon::start();
    daemon.call("workspace.create", &json!({ "cwd": "/tmp", "label": "silent", "focus": true }));
    let pane = only_pane(&daemon);
    let client = HerdrClient::new(daemon.socket_path().to_string_lossy().into_owned());

    let never = json!({
        "pane_id": pane,
        "match": { "type": "regex", "value": "zzz-nothing-prints-this-zzz" },
        "source": "visible",
        "timeout_ms": 300,
    });
    let failure = client
        .request_within("pane.wait_for_output", &never, Duration::from_secs(10))
        .expect_err("nothing prints that");
    assert!(
        matches!(&failure, Failure::Daemon { code, .. } if code == "timeout"),
        "a wait that found nothing has to be distinguishable from a daemon that went away, \
         because one means send anyway and the other means stop. Got: {failure:?}"
    );
}

fn only_pane(daemon: &Daemon) -> String {
    let snapshot = daemon.call("session.snapshot", &json!({}));
    let panes = snapshot["snapshot"]["panes"]
        .as_array()
        .unwrap_or_else(|| panic!("no panes in {snapshot}"));
    assert_eq!(panes.len(), 1, "a fresh workspace holds one pane, and held {panes:?}");
    panes[0]["pane_id"].as_str().expect("a pane carries an id").to_string()
}

/// One request, spelled by hand, so this test can do the thing the client will not.
///
/// `None` is an answer that never arrived.
fn raw_call(
    socket: &std::path::Path,
    method: &str,
    params: &Value,
    half_closed: bool,
) -> Option<String> {
    let mut stream = UnixStream::connect(socket).expect("the daemon is listening");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("a socket takes a deadline");
    let line = json!({ "id": "test", "method": method, "params": params }).to_string() + "\n";
    stream.write_all(line.as_bytes()).expect("the daemon takes a request");
    if half_closed {
        stream.shutdown(std::net::Shutdown::Write).expect("a socket can be half-closed");
    }

    let mut answer = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) == 1 {
        if byte[0] == b'\n' {
            break;
        }
        answer.push(byte[0]);
    }
    (!answer.is_empty()).then(|| String::from_utf8_lossy(&answer).to_string())
}
