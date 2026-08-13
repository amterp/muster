//! What each request means.
//!
//! Bytes in, bytes out, no FFI: this is where the seam's behavior can be tested without a
//! shell, a window or a linker. [`crate::ffi`] is the shim that lets a C caller reach it.

use std::collections::BTreeMap;

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::diagnostics::sink::JsonLinesSink;
use muster_core::fields;

use crate::proto::{self, Request, Response, request};
use prost::Message;

/// Answers one encoded request.
///
/// Total by construction. A request that will not decode is still answered, because the
/// alternative is a shell that cannot tell "the core refused" from "the core is gone", and
/// those want very different reactions.
pub fn dispatch(request: &[u8]) -> Vec<u8> {
    let response = match Request::decode(request) {
        Ok(request) => handle(request),
        Err(error) => Response::failure(format!(
            "the core could not decode a request ({error}). Whatever the shell was asking \
             for did not happen and will not be retried. The two sides are generated from \
             one proto/muster.proto in one build, so this means a stale libmuster.dylib \
             rather than a schema disagreement - check that ./dev built the core this \
             shell is linked against."
        )),
    };
    response.encode_to_vec()
}

fn handle(request: Request) -> Response {
    let Some(payload) = request.payload else {
        return Response::failure(
            "the core was handed a request with no payload, so there was nothing to do. \
             This is a bug in the shell's request building rather than a state worth \
             recovering from; the field is a oneof and every arm sets it.",
        );
    };

    match payload {
        request::Payload::Startup(startup) => start(&startup),
        request::Payload::LogRecord(record) => write(record),
    }
}

/// Turns logging on, if this run wants it.
///
/// An empty path is not a failure: it is what a release build asks for, and what the
/// shell sends when the user has not opted in.
fn start(startup: &proto::Startup) -> Response {
    if startup.log_path.is_empty() {
        return Response::ok();
    }
    let Some(sink) = JsonLinesSink::open(&startup.log_path) else {
        return Response::failure(format!(
            "the core could not open {} for logging, so this run leaves no record. \
             Everything else works; a bug report from it will just be missing the timeline \
             that usually explains what happened. Check that the directory exists and is \
             writable.",
            startup.log_path
        ));
    };
    let level = if startup.log_level.is_empty() {
        LogLevel::Debug
    } else {
        match LogLevel::parse(&startup.log_level) {
            Some(level) => level,
            None => {
                return Response::failure(format!(
                    "the core does not know a log level called {:?}, so logging stayed off \
                     and this run leaves no record. Valid levels are trace, debug, info, \
                     warn and error; check MUSTER_LOG_LEVEL.",
                    startup.log_level
                ));
            }
        }
    };
    let process =
        if startup.process.is_empty() { "app".to_string() } else { startup.process.clone() };
    log::install(Box::new(sink), process, level);

    // First record of the run, and the one a bug report is read against: which engine will
    // encode this session's keystrokes. libghostty-vt is reproduced from deps/ghostty.pin
    // rather than installed, so "the pin in the repo today" is not an answer about a run
    // from last week.
    log::info(
        "core.start",
        fields! {
            "vt_engine" => muster_vt::engine_version().unwrap_or_else(|| "unknown".to_string()),
        },
    );
    Response::ok()
}

fn write(record: proto::LogRecord) -> Response {
    let Some(level) = LogLevel::parse(&record.level) else {
        return Response::failure(format!(
            "the core does not know a log level called {:?}, so the {:?} record was \
             dropped. Whatever it was reporting is now invisible. The shell builds this \
             field from a fixed set, so this is a bug there rather than a configuration \
             problem.",
            record.level, record.event
        ));
    };
    // The map arrives unordered, as protobuf maps do, and records are sorted so that two
    // runs of the same code produce the same bytes.
    log::emit(level, &record.event, record.fields.into_iter().collect::<BTreeMap<_, _>>());
    Response::ok()
}
