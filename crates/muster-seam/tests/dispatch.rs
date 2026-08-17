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
    let _turn = muster::testing::fresh_session();
    // The release default. Logging off is a choice, not a failure, and a seam that
    // reported it as one would put an error in every release run's stderr.
    let response =
        answer(&Request { payload: Some(request::Payload::Startup(Startup::default())) });

    assert!(is_ok(&response), "{response:?}");
}

#[test]
fn a_log_path_that_cannot_be_opened_says_what_it_costs() {
    let _turn = muster::testing::fresh_session();
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
    let _turn = muster::testing::fresh_session();
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
    let _turn = muster::testing::fresh_session();
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
    let _turn = muster::testing::fresh_session();
    // The failure mode this guards against is a shell linked against a stale dylib: the
    // bytes decode as nothing, and without an answer the shell cannot tell that from a
    // core that has stopped responding at all.
    let response =
        Response::decode(dispatch(&[0xff, 0xff, 0xff, 0xff]).as_slice()).expect("still a response");

    let reason = failure(&response);
    assert!(reason.contains("libmuster.dylib"), "{reason}");
}

/// A turn starts where a fresh process would, which is what lets a binary hold more than one.
///
/// Bindings because they are the leak somebody would notice: a config file read by the last
/// test is a chord meaning what somebody else configured in this one. The state path is the
/// leak that would be worse and cannot be checked from here - it takes a daemon to make a
/// window write anything - so this is the observable end of the same rule.
///
/// Two turns in one test rather than two tests, deliberately: what is under test is the
/// boundary between them, and a pair of tests would only assert it in whatever order the
/// binary happened to run them.
#[test]
fn a_fresh_session_starts_where_a_new_process_would() {
    let config = std::env::temp_dir().join(format!("muster-reset-{}.toml", std::process::id()));
    std::fs::write(&config, "[keymap]\nzoom = \"cmd+shift+z\"\n")
        .unwrap_or_else(|e| panic!("could not write {}: {e}", config.display()));

    let turn = muster::testing::fresh_session();
    answer(&Request {
        payload: Some(request::Payload::Startup(Startup {
            config_path: config.to_string_lossy().into_owned(),
            ..Startup::default()
        })),
    });
    assert_eq!(
        zoom_chord(),
        Some("KeyZ".to_string()),
        "the config file naming this chord was not applied, so what follows would prove nothing"
    );
    drop(turn);

    let _turn = muster::testing::fresh_session();
    assert_eq!(
        zoom_chord(),
        Some("Enter".to_string()),
        "the last test's config file is still in force, so a window in this one binds a chord \
         nobody here configured"
    );
    let _ = std::fs::remove_file(&config);
}

/// Which key zooms, as the core would tell a shell building its menu.
fn zoom_chord() -> Option<String> {
    let response = answer(&Request {
        payload: Some(request::Payload::ReadBindings(muster::proto::ReadBindings {})),
    });
    match response.payload {
        Some(response::Payload::Bindings(bindings)) => bindings
            .bindings
            .into_iter()
            .find(|binding| binding.action == "zoom")
            .map(|binding| binding.key),
        other => panic!("expected the bindings, got {other:?}"),
    }
}

#[test]
fn an_empty_request_is_answered() {
    let _turn = muster::testing::fresh_session();
    // Zero bytes decode cleanly as a Request with no payload set, so this is not the
    // malformed case - it is the one where the shell built an envelope and put nothing in
    // it, which is a different bug with a different cure.
    let response = Response::decode(dispatch(&[]).as_slice()).expect("still a response");

    let reason = failure(&response);
    assert!(reason.contains("no payload"), "{reason}");
}
