use std::io;
use std::sync::TryLockError;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use proto::{answer::Detail, pane_request};

use super::{Document, Stage, WriteError, validate};
use crate::api::Cause;
use crate::session::{Reply, Session, Shared};

#[derive(Debug, Clone, Copy)]
pub(crate) enum FailureKind {
    Request,
    Unavailable,
    Io,
    DurabilityUnknown,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) reply: Reply,
    pub(crate) kind: FailureKind,
}

impl Failure {
    pub(crate) fn request(reply: Reply) -> Self {
        Self { reply, kind: FailureKind::Request }
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Self::request(Reply::refused(reason))
    }

    pub(crate) fn unavailable(reason: &str) -> Self {
        Self { reply: Reply::refused(reason), kind: FailureKind::Unavailable }
    }

    fn io(reason: String) -> Self {
        Self { reply: Reply::refused(reason), kind: FailureKind::Io }
    }
}

pub(crate) type Answer = Result<Reply, Failure>;

pub(crate) fn serve(
    shared: &Shared,
    request: pane_request::Request,
    cause: Option<&Cause>,
    answer: impl FnOnce(Answer),
) {
    let _operation = shared.canvases.operation();
    match request {
        pane_request::Request::CreateCanvas(asked) => {
            answer(create(shared, asked, cause, |_, answer| answer));
        }
        pane_request::Request::WriteCanvas(write) => {
            access(shared, &write.pane, answer, |serial, recovering| {
                written(shared, &write.pane, serial, recovering, write.document, cause)
            });
        }
        pane_request::Request::ReadCanvas(read) => {
            access(shared, &read.pane, answer, |serial, recovering| {
                read_back(shared, &read.pane, serial, recovering)
            });
        }
        _ => unreachable!("only the canvas requests are served here"),
    }
}

/// The caller holds the canvas lifetime guard through the answer. Finish keeps an API creation's
/// operation record and placement in the same session snapshot.
pub(crate) fn create<T>(
    shared: &Shared,
    create: pane_request::CreateCanvas,
    cause: Option<&Cause>,
    finish: impl FnOnce(&mut Session, Answer) -> T,
) -> T {
    let starting = {
        let mut session = shared.lock();
        match session.prepare_canvas(create) {
            Ok(starting) => starting,
            Err(refused) => return finish(&mut session, Err(refused)),
        }
    };
    let stored = validate(&starting.document.content).map_err(Failure::invalid).and_then(|()| {
        shared
            .canvases
            .write(starting.name(), &starting.document, true, |stage| fault(shared, stage, true))
            .map_err(|failed| write_refusal(starting.name(), &failed))
    });
    let mut session = shared.lock();
    session.cause = cause.cloned();
    let answer = session.canvas_started(starting, stored);
    session.cause = None;
    finish(&mut session, answer)
}

fn access(
    shared: &Shared,
    name: &str,
    answer: impl FnOnce(Answer),
    run: impl FnOnce(u64, &mut bool) -> Answer,
) {
    let target = shared.lock().canvas_target(name);
    let (serial, canvas) = match target {
        Ok(target) => target,
        Err(refused) => return answer(Err(refused)),
    };
    let mut recovering = match canvas.recovering.try_lock() {
        Ok(recovering) => recovering,
        Err(error) => {
            if matches!(error, TryLockError::WouldBlock) {
                log::debug("daemon.canvas.document_wait", fields! { "pane" => name });
            }
            poison::lock(&canvas.recovering, "daemon.canvas")
        }
    };
    let current = shared.lock().canvas_target(name);
    let reply = match current {
        Ok((held, _)) if held == serial => run(serial, &mut recovering),
        Ok(_) => Err(Failure::request(Reply::not_there(format!(
            "canvas {name} closed while the request waited"
        )))),
        Err(refused) => Err(refused),
    };
    answer(reply);
}

fn read_back(shared: &Shared, name: &str, serial: u64, recovering: &mut bool) -> Answer {
    let document = read_document(shared, name, serial, recovering, None)?;
    Ok(Reply {
        detail: Some(Box::new(Detail::CanvasRead(proto::CanvasRead {
            revision: document.revision,
            document: Some(document.content),
        }))),
        ..Reply::done()
    })
}

fn read_document(
    shared: &Shared,
    name: &str,
    serial: u64,
    recovering: &mut bool,
    cause: Option<&Cause>,
) -> Result<Document, Failure> {
    let document = shared
        .canvases
        .read(name)
        .map_err(|error| Failure::io(format!("could not read canvas {name}: {error}")))?;
    if *recovering {
        changed(shared, name, serial, &document, cause)?;
        *recovering = false;
    }
    Ok(document)
}

fn changed(
    shared: &Shared,
    name: &str,
    serial: u64,
    document: &Document,
    cause: Option<&Cause>,
) -> Result<(), Failure> {
    let mut session = shared.lock();
    session.cause = cause.cloned();
    let changed = session.canvas_changed(name, serial, document);
    session.cause = None;
    changed.map_err(Failure::request)
}

fn written(
    shared: &Shared,
    name: &str,
    serial: u64,
    recovering: &mut bool,
    content: Option<proto::CanvasDocument>,
    cause: Option<&Cause>,
) -> Answer {
    if *recovering {
        read_document(shared, name, serial, recovering, None)?;
    }
    let content =
        content.ok_or_else(|| Failure::invalid("the request carries no canvas document"))?;
    validate(&content).map_err(Failure::invalid)?;
    let revision = shared
        .lock()
        .canvas_record(name, serial)
        .map_err(Failure::request)?
        .revision
        .checked_add(1)
        .ok_or_else(|| Failure::invalid(format!("canvas {name} has no revisions left")))?;
    let document = Document { revision, content };
    match shared.canvases.write(name, &document, false, |stage| fault(shared, stage, false)) {
        Ok(()) => changed(shared, name, serial, &document, cause)?,
        Err(failed) => {
            if failed.stage == Stage::AfterRename {
                *recovering = true;
                if let Err(refused) = read_document(shared, name, serial, recovering, cause) {
                    log::warn(
                        "daemon.canvas.recovery_pending",
                        fields! { "pane" => name, "why" => refused.reply.reason },
                    );
                }
            }
            return Err(write_refusal(name, &failed));
        }
    }
    Ok(Reply {
        detail: Some(Box::new(Detail::CanvasWritten(proto::CanvasWritten { revision }))),
        ..Reply::done()
    })
}

fn write_refusal(name: &str, failed: &WriteError) -> Failure {
    let error = &failed.error;
    match failed.stage {
        Stage::BeforeRename => Failure::io(format!("could not save canvas {name}: {error}")),
        Stage::AfterRename => Failure {
            kind: FailureKind::DurabilityUnknown,
            reply: Reply::refused(format!(
                "could not sync canvas {name} after replacing its file: {error}. \
                 The durability of this write is unknown."
            )),
        },
    }
}

fn fault(shared: &Shared, stage: Stage, creating: bool) -> io::Result<()> {
    if !cfg!(debug_assertions) {
        return Ok(());
    }
    let Some(fault) = std::env::var_os("MUSTER_DAEMON_CANVAS_FAULT") else {
        return Ok(());
    };
    if creating != (fault == "pause-before-create-rename") {
        return Ok(());
    }
    match stage {
        Stage::BeforeRename if fault == "fail-before-rename" => {
            Err(io::Error::other("injected canvas failure before rename"))
        }
        Stage::AfterRename if fault == "fail-after-rename" => {
            Err(io::Error::other("injected canvas directory sync failure"))
        }
        Stage::BeforeRename if fault == "exit-before-rename" => std::process::exit(97),
        Stage::BeforeRename
            if fault == "pause-before-rename" || fault == "pause-before-create-rename" =>
        {
            let paused = shared.socket.path.with_extension("canvas-pause");
            let resume = shared.socket.path.with_extension("canvas-resume");
            std::fs::write(paused, "before rename\n")?;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !resume.exists() {
                if std::time::Instant::now() >= until {
                    return Err(io::Error::other("the canvas test did not release its pause"));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
