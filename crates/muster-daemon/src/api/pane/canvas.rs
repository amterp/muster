use muster_api as api;
use muster_core::names::{Mint, Minter};
use muster_daemon_proto as proto;
use prost::Message;

use super::super::{Caller, refused};
use super::{Operation, begun, made_answer, unknown};
use crate::api::Cause;
use crate::canvas::{self, Failure, FailureKind};
use crate::operations;
use crate::persist::Made;
use crate::session::Shared;

const UNCERTAIN_CREATE: &str = "a request with this operation id was in progress when the daemon stopped. Its result is \
     unknown. Asking with this id returns the same outcome while the daemon remembers it";

pub(in crate::api) fn new(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: api::NewCanvas,
) {
    let fingerprint = {
        let mut bytes = b"pane.new_canvas\0".to_vec();
        asked.encode(&mut bytes).expect("encoding a canvas request into a Vec");
        operations::fingerprint(&bytes)
    };
    let _canvas = shared.canvases.operation();
    let answer =
        made(shared, caller, correlation, asked, fingerprint).unwrap_or_else(|answer| *answer);
    let _session = shared.lock();
    shared.api.answer(caller.connection, answer);
}

fn made(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: api::NewCanvas,
    fingerprint: u64,
) -> Result<api::Answer, Box<api::Answer>> {
    let api::NewCanvas { operation: id, name, document, opened_by, placement } = asked;
    let operation = Operation {
        client: &caller.client,
        id: &id,
        cause: Cause { connection: caller.connection, correlation },
    };
    operation.check_id("a new canvas", "nothing was made")?;
    begun(&mut shared.lock(), &operation, fingerprint, "nothing was made", UNCERTAIN_CREATE)?;

    let mut drawn = Minter::new(Mint::Drawn);
    let pane = drawn.pane().to_string();
    let placement = placement.map(|placement| proto::Placement {
        r#where: Some(match placement {
            api::new_canvas::Placement::NewTab(_) => {
                proto::placement::Where::NewTab(proto::placement::NewTab {
                    tab: drawn.tab().to_string(),
                    ..Default::default()
                })
            }
            api::new_canvas::Placement::Beside(beside) => {
                let side = match beside.side() {
                    api::new_canvas::Side::Left => proto::Side::Left,
                    api::new_canvas::Side::Right => proto::Side::Right,
                    api::new_canvas::Side::Up => proto::Side::Up,
                    api::new_canvas::Side::Down => proto::Side::Down,
                    api::new_canvas::Side::Unspecified => proto::Side::Unspecified,
                };
                proto::placement::Where::Beside(proto::placement::Beside {
                    pane: beside.pane,
                    side: side.into(),
                    ratio: beside.ratio,
                })
            }
        }),
    });
    let create = proto::pane_request::CreateCanvas {
        pane: pane.clone(),
        placement,
        label: name.filter(|name| !name.is_empty()),
        document: document.map(internal_document),
        opened_by,
        ..Default::default()
    };
    canvas::create(shared, create, Some(&operation.cause), |session, response| {
        let correlation = operation.cause.correlation.clone();
        let answer = match response {
            Ok(reply) => {
                let Some(proto::answer::Detail::Created(created)) =
                    reply.detail.map(|detail| *detail)
                else {
                    unreachable!("a successful canvas create carries Created");
                };
                let made = Made { pane, callsign: created.callsign };
                session.pane_operations.done(operation.client, operation.id, made.clone());
                made_answer(correlation, api::Outcome::Done, made)
            }
            Err(mut failure) => {
                operation.forget(session);
                if matches!(failure.kind, FailureKind::DurabilityUnknown) {
                    // A failed create never publishes a pane, even if it left a file to collect.
                    failure.kind = FailureKind::Io;
                }
                failed(correlation, failure)
            }
        };
        session.state_changed();
        Ok(answer)
    })
}

pub(in crate::api) fn write(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: api::WriteCanvas,
) {
    let write = proto::pane_request::WriteCanvas {
        pane: asked.pane,
        document: asked.document.map(internal_document),
    };
    access(shared, caller, correlation, proto::pane_request::Request::WriteCanvas(write));
}

pub(in crate::api) fn read(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    asked: api::ReadCanvas,
) {
    let read = proto::pane_request::ReadCanvas { pane: asked.pane };
    access(shared, caller, correlation, proto::pane_request::Request::ReadCanvas(read));
}

fn access(
    shared: &Shared,
    caller: &Caller,
    correlation: String,
    request: proto::pane_request::Request,
) {
    let cause = Cause { connection: caller.connection, correlation };
    canvas::serve(shared, request, Some(&cause), |response| {
        let correlation = cause.correlation.clone();
        let answer = match response {
            Ok(reply) => api::Answer {
                correlation,
                outcome: api::Outcome::Done.into(),
                detail: reply.detail.map(|detail| match *detail {
                    proto::answer::Detail::CanvasWritten(written) => {
                        api::answer::Detail::CanvasWritten(api::CanvasWritten {
                            revision: written.revision,
                        })
                    }
                    proto::answer::Detail::CanvasRead(read) => {
                        api::answer::Detail::CanvasRead(api::CanvasRead {
                            revision: read.revision,
                            document: read.document.map(public_document),
                        })
                    }
                    other => unreachable!("a canvas read or write cannot answer {other:?}"),
                }),
                ..Default::default()
            },
            Err(failure) => failed(correlation, failure),
        };
        shared.api.answer(caller.connection, answer);
    });
}

fn failed(correlation: String, failure: Failure) -> api::Answer {
    let reason = failure.reply.reason;
    let refusal = match failure.kind {
        FailureKind::Request => match failure.reply.outcome {
            proto::Outcome::NotThere => api::Refusal::NotFound,
            proto::Outcome::AlreadySo => {
                return refused(correlation, api::Refusal::Unavailable, "the canvas id is in use");
            }
            _ => api::Refusal::Invalid,
        },
        FailureKind::Unavailable => api::Refusal::Unavailable,
        FailureKind::Io => api::Refusal::Unspecified,
        FailureKind::DurabilityUnknown => return unknown(correlation, &reason),
    };
    refused(correlation, refusal, reason)
}

fn internal_document(document: api::CanvasDocument) -> proto::CanvasDocument {
    let kind = match document.kind() {
        api::CanvasKind::Unspecified => proto::CanvasKind::Unspecified,
        api::CanvasKind::Markdown => proto::CanvasKind::Markdown,
        api::CanvasKind::Html => proto::CanvasKind::Html,
        api::CanvasKind::Image => proto::CanvasKind::Image,
    };
    proto::CanvasDocument {
        kind: kind.into(),
        body: document.body,
        image_type: document.image_type,
        title: document.title,
    }
}

fn public_document(document: proto::CanvasDocument) -> api::CanvasDocument {
    api::CanvasDocument {
        kind: public_kind(document.kind()).into(),
        body: document.body,
        image_type: document.image_type,
        title: document.title,
    }
}

pub(in crate::api) fn public_kind(kind: proto::CanvasKind) -> api::CanvasKind {
    match kind {
        proto::CanvasKind::Unspecified => api::CanvasKind::Unspecified,
        proto::CanvasKind::Markdown => api::CanvasKind::Markdown,
        proto::CanvasKind::Html => api::CanvasKind::Html,
        proto::CanvasKind::Image => api::CanvasKind::Image,
    }
}
