//! What each request means.
//!
//! Bytes in, bytes out, no FFI: this is where the seam's behavior can be tested without a
//! shell, a window or a linker. [`crate::ffi`] is the shim that lets a C caller reach it.

use std::collections::BTreeMap;

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::diagnostics::sink::JsonLinesSink;
use muster_core::fields;

use muster_core::input::{CompositionOutcome, ScrollDirection, composition_outcome};

use crate::convert;
use crate::proto::{self, Request, Response, request, response};
use crate::session::{self, AttachError, AttachedPane};
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
        request::Payload::AttachPane(attach) => attach_pane(&attach.pane_id),
        request::Payload::KeyDown(down) => with_pane("a keystroke", |pane| key_down(pane, &down)),
        request::Payload::KeyUp(up) => {
            with_pane("a key release", |pane| send_key(pane, up.key.as_ref()))
        }
        request::Payload::SendText(text) => with_pane("text", |pane| {
            pane.input.send_text(&text.text);
            Response::ok()
        }),
        request::Payload::Paste(paste) => with_pane("a paste", |pane| {
            pane.input.paste(&paste.text);
            Response::ok()
        }),
        request::Payload::Scroll(scroll) => {
            with_pane("a scroll", |pane| match ScrollDirection::parse(&scroll.direction) {
                Some(direction) => {
                    pane.input.scroll(direction, scroll.lines.try_into().unwrap_or(u16::MAX));
                    Response::ok()
                }
                None => Response::failure(format!(
                    "the core does not know a scroll direction called {:?}, so the wheel did \
                     nothing. Only up and down exist; the shell builds this from a fixed set, \
                     so this is a bug there.",
                    scroll.direction
                )),
            })
        }
    }
}

/// One press, after the input method has had its turn.
///
/// The arbitration happens here rather than in the shell because it is a decision, and
/// exactly one thing may come of a press: the text a composition produced, or the key
/// itself, or nothing at all.
fn key_down(pane: &AttachedPane, down: &proto::KeyDown) -> Response {
    match composition_outcome(down.was_composing, down.committed.as_deref(), down.still_composing) {
        CompositionOutcome::SendNothing => Response::ok(),
        CompositionOutcome::SendText(text) => {
            pane.input.send_text(&text);
            Response::ok()
        }
        CompositionOutcome::SendKey => send_key(pane, down.key.as_ref()),
    }
}

fn send_key(pane: &AttachedPane, key: Option<&proto::KeyEvent>) -> Response {
    let Some(key) = key else {
        return Response::failure(
            "the core was handed a keystroke with no key in it, so nothing reached the pane. \
             This is a bug in the shell's request building rather than a state worth \
             recovering from.",
        );
    };
    match convert::key(key) {
        Ok(key) => {
            pane.input.send(&key);
            Response::ok()
        }
        Err(reason) => Response::failure(reason),
    }
}

/// Runs something against the pane the keyboard feeds, or explains why there is not one.
///
/// A bare `muster` legitimately has no pane - it is the renderer check - so this is a
/// refusal rather than an error, and it says which input went nowhere so a log reads as
/// something other than silence.
///
/// The session's lock is released before `act` runs. Sending can be a round trip to a
/// daemon, and holding the session across one would stall every event arriving from every
/// other daemon behind a wedged one.
fn with_pane(what: &str, act: impl FnOnce(&AttachedPane) -> Response) -> Response {
    match session::keyboard_pane() {
        Some(pane) => act(&pane),
        None => Response::failure(format!(
            "no pane has this window's keyboard, so {what} went nowhere. A window with no \
             pane named is the renderer check and this is expected there; anywhere else it \
             means the attach failed earlier, or the pane it succeeded on has since exited, \
             and that is the event worth reading."
        )),
    }
}

fn attach_pane(pane_id: &str) -> Response {
    match session::attach(pane_id) {
        Ok(pane) => Response {
            payload: Some(response::Payload::Attached(proto::Attached {
                control_socket_path: pane.control_socket_path.clone(),
                server_encoded: pane.server_encoded,
            })),
        },
        Err(AttachError::NoDaemon) => Response::failure(
            "no herdr daemon could be found, so there is no pane to attach to and this \
             window will render nothing. Every pane Muster shows is owned by a daemon; \
             check that one is running, and that HERDR_SOCKET_PATH or the default socket \
             path points at it.",
        ),
        Err(AttachError::Unreachable(detail)) => Response::failure(format!(
            "the daemon did not answer ({detail}), so nothing is known about this pane and \
             the window will render nothing. The socket exists, which usually means a \
             daemon that is wedged or shutting down rather than one that is absent."
        )),
        Err(AttachError::NoSuchPane { pane, held, dropped }) => Response::failure(format!(
            "no daemon holds a pane called {pane}, so this window has nothing to show and \
             would render and ignore the keyboard. The daemon answered with {held} pane(s); \
             `herdr pane list` names them. {dropped} entries in that answer did not read, \
             so if this id is one of them the problem is a herdr whose payload has moved \
             rather than a pane that is missing."
        )),
        Err(AttachError::NoSocket(detail)) => Response::failure(format!(
            "the core could not open the socket a pane's bridge dials back on ({detail}), so \
             this pane can never be typed into - it will render and ignore the keyboard. \
             Usual causes: a full or read-only temporary directory."
        )),
        Err(AttachError::NoEncoder(detail)) => Response::failure(format!(
            "the core could not build a key encoder ({detail}), so nothing typed into this \
             pane would reach it. libghostty-vt is behind this; check that ./dev built it."
        )),
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
