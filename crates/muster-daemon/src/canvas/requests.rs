use std::io;
use std::sync::TryLockError;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use proto::{answer::Detail, pane_request};

use super::{Document, Stage, WriteError, validate};
use crate::session::{Reply, Shared};

pub(crate) fn serve(shared: &Shared, request: pane_request::Request, answer: impl FnOnce(Reply)) {
    let _operation = shared.canvases.operation();
    match request {
        pane_request::Request::CreateCanvas(create) => answer(created(shared, create)),
        pane_request::Request::WriteCanvas(write) => {
            access(shared, &write.pane, answer, |serial, recovering| {
                written(shared, &write.pane, serial, recovering, write.document)
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

fn created(shared: &Shared, create: pane_request::CreateCanvas) -> Reply {
    let starting = shared.lock().prepare_canvas(create);
    let starting = match starting {
        Ok(starting) => starting,
        Err(refused) => return refused,
    };
    let stored = validate(&starting.document.content).map_err(Reply::refused).and_then(|()| {
        shared
            .canvases
            .write(starting.name(), &starting.document, true, |stage| fault(shared, stage, true))
            .map_err(|failed| write_refusal(starting.name(), &failed))
    });
    shared.lock().canvas_started(starting, stored)
}

fn access(
    shared: &Shared,
    name: &str,
    answer: impl FnOnce(Reply),
    run: impl FnOnce(u64, &mut bool) -> Reply,
) {
    let target = shared.lock().canvas_target(name);
    let (serial, canvas) = match target {
        Ok(target) => target,
        Err(refused) => return answer(refused),
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
        Ok(_) => Reply::not_there(format!("canvas {name} closed while the request waited")),
        Err(refused) => refused,
    };
    answer(reply);
}

fn read_back(shared: &Shared, name: &str, serial: u64, recovering: &mut bool) -> Reply {
    match read_document(shared, name, serial, recovering) {
        Ok(document) => Reply {
            detail: Some(Box::new(Detail::CanvasRead(proto::CanvasRead {
                revision: document.revision,
                document: Some(document.content),
            }))),
            ..Reply::done()
        },
        Err(refused) => refused,
    }
}

fn read_document(
    shared: &Shared,
    name: &str,
    serial: u64,
    recovering: &mut bool,
) -> Result<Document, Reply> {
    let document = shared
        .canvases
        .read(name)
        .map_err(|error| Reply::refused(format!("could not read canvas {name}: {error}")))?;
    if *recovering {
        shared.lock().canvas_changed(name, serial, &document)?;
        *recovering = false;
    }
    Ok(document)
}

fn written(
    shared: &Shared,
    name: &str,
    serial: u64,
    recovering: &mut bool,
    content: Option<proto::CanvasDocument>,
) -> Reply {
    let written = (|| {
        if *recovering {
            read_document(shared, name, serial, recovering)?;
        }
        let content =
            content.ok_or_else(|| Reply::refused("the request carries no canvas document"))?;
        validate(&content).map_err(Reply::refused)?;
        let revision = shared
            .lock()
            .canvas_record(name, serial)?
            .revision
            .checked_add(1)
            .ok_or_else(|| Reply::refused(format!("canvas {name} has no revisions left")))?;
        let document = Document { revision, content };
        match shared.canvases.write(name, &document, false, |stage| fault(shared, stage, false)) {
            Ok(()) => shared.lock().canvas_changed(name, serial, &document)?,
            Err(failed) => {
                if failed.stage == Stage::AfterRename {
                    *recovering = true;
                    if let Err(refused) = read_document(shared, name, serial, recovering) {
                        log::warn(
                            "daemon.canvas.recovery_pending",
                            fields! { "pane" => name, "why" => refused.reason },
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
    })();
    written.unwrap_or_else(|refused| refused)
}

fn write_refusal(name: &str, failed: &WriteError) -> Reply {
    let error = &failed.error;
    match failed.stage {
        Stage::BeforeRename => Reply::refused(format!("could not save canvas {name}: {error}")),
        Stage::AfterRename => Reply::refused(format!(
            "could not sync canvas {name} after replacing its file: {error}. \
             The durability of this write is unknown."
        )),
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
