//! What the seam does with what it is handed.
//!
//! The FFI shim is deliberately too thin to hold behavior, so this is where the seam's
//! behavior gets asserted - with no linker, no shell, and no window in the picture.
//!
//! The theme running through these: a seam that answers nothing is indistinguishable from
//! a core that has died, and the shell reacts to those very differently. So every case
//! here is about the answer coming back, including the cases where the answer is no.

use muster::dispatch;
use muster::proto::{LogRecord, Request, Response, Startup, request, response};
use prost::Message;

fn answer(request: &Request) -> Response {
    Response::decode(dispatch(&request.encode_to_vec()).as_slice()).expect("a response decodes")
}

fn failure(response: &Response) -> &str {
    match response.payload.as_ref() {
        Some(response::Payload::Failure(failure)) => &failure.reason,
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn is_ok(response: &Response) -> bool {
    matches!(response.payload, Some(response::Payload::Ok(_)))
}

#[test]
fn a_startup_with_no_log_path_is_accepted_rather_than_refused() {
    // The release default. Logging off is a choice, not a failure, and a seam that
    // reported it as one would put an error in every release run's stderr.
    let response =
        answer(&Request { payload: Some(request::Payload::Startup(Startup::default())) });

    assert!(is_ok(&response), "{response:?}");
}

#[test]
fn a_log_path_that_cannot_be_opened_says_what_it_costs() {
    let response = answer(&Request {
        payload: Some(request::Payload::Startup(Startup {
            log_path: "/nonexistent-directory-for-a-test/muster.jsonl".to_string(),
            ..Startup::default()
        })),
    });

    let reason = failure(&response);
    // Named, so an investigator does not have to guess which path was tried.
    assert!(reason.contains("/nonexistent-directory-for-a-test/muster.jsonl"), "{reason}");
    // And the impact, because the whole point of the log is being missing exactly when
    // someone needs it.
    assert!(reason.contains("no record"), "{reason}");
}

#[test]
fn a_level_the_core_does_not_know_is_refused_by_name() {
    let response = answer(&Request {
        payload: Some(request::Payload::LogRecord(LogRecord {
            level: "verbose".to_string(),
            event: "app.launch".to_string(),
            ..LogRecord::default()
        })),
    });

    let reason = failure(&response);
    assert!(reason.contains("verbose"), "{reason}");
    // The dropped record's own name, so the reader knows what went missing rather than
    // only that something did.
    assert!(reason.contains("app.launch"), "{reason}");
}

#[test]
fn a_record_at_a_known_level_is_accepted() {
    // Logging is off in this process, so nothing is written - and that is the point:
    // "accepted" must not mean "a sink happened to be installed".
    let response = answer(&Request {
        payload: Some(request::Payload::LogRecord(LogRecord {
            level: "info".to_string(),
            event: "app.ready".to_string(),
            fields: [("typeable".to_string(), "true".to_string())].into_iter().collect(),
        })),
    });

    assert!(is_ok(&response), "{response:?}");
}

#[test]
fn bytes_that_are_not_a_request_are_answered_rather_than_dropped() {
    // The failure mode this guards against is a shell linked against a stale dylib: the
    // bytes decode as nothing, and without an answer the shell cannot tell that from a
    // core that has stopped responding at all.
    let response =
        Response::decode(dispatch(&[0xff, 0xff, 0xff, 0xff]).as_slice()).expect("still a response");

    let reason = failure(&response);
    assert!(reason.contains("libmuster.dylib"), "{reason}");
}

#[test]
fn an_empty_request_is_answered() {
    // Zero bytes decode cleanly as a Request with no payload set, so this is not the
    // malformed case - it is the one where the shell built an envelope and put nothing in
    // it, which is a different bug with a different cure.
    let response = Response::decode(dispatch(&[]).as_slice()).expect("still a response");

    let reason = failure(&response);
    assert!(reason.contains("no payload"), "{reason}");
}
