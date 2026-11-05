use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use muster_core::PaneKey;
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::input::image::{MAX_IMAGE_BYTES, check_png};
use muster_core::input::{ImagePasteError, ImageUpload, Key, KeyAction, Modifiers};

use crate::proto::{self, Event, Response, event, response};
use crate::session::{self, WindowId};
use crate::{convert, ffi};

static BUDGET: Mutex<(usize, usize)> = Mutex::new((0, 0));
static NEXT: AtomicU64 = AtomicU64::new(1);
static PREPARED: LazyLock<Mutex<BTreeMap<u64, Prepared>>> = LazyLock::new(Mutex::default);

struct Prepared {
    destination: session::ImageDestination,
    upload: ImageUpload,
    lease: UploadBytes,
    deadline: mpsc::Sender<()>,
}

struct UploadBytes(usize);

impl UploadBytes {
    fn take(bytes: usize) -> Result<UploadBytes, String> {
        let mut budget = poison::lock(&BUDGET, "image upload budget");
        if budget.0 >= 2 || budget.1 + bytes > 32 << 20 {
            return Err("other image pastes are still running; try again when one finishes".into());
        }
        budget.0 += 1;
        budget.1 += bytes;
        Ok(UploadBytes(bytes))
    }
}

impl Drop for UploadBytes {
    fn drop(&mut self) {
        let mut budget = poison::lock(&BUDGET, "image upload budget");
        budget.0 -= 1;
        budget.1 -= self.0;
    }
}

pub(crate) fn target(window: WindowId, request: &proto::ReadImagePasteTarget) -> Response {
    let target = || {
        if request.composing {
            return Err("an input composition owns the key");
        }
        let key = request
            .key
            .as_ref()
            .and_then(|key| convert::key(key).ok())
            .ok_or("the key is missing or unknown")?;
        if key.key != Key::KeyV
            || key.modifiers.intersection(Modifiers::CHORD) != Modifiers::CONTROL
            || key.action == KeyAction::Release
        {
            return Err("the key is not Ctrl+V");
        }
        let destination = session::image_destination(window, None).ok_or("the pane is gone")?;
        if !destination.remote {
            return Err("the pane is local and reads its own clipboard");
        }
        if !destination.connected {
            return Err("the pane's daemon is disconnected");
        }
        if !destination.ready {
            return Err("no supported image composer is readable");
        }
        let pane = session::attached_pane(&destination.key.daemon, &destination.key.pane)
            .ok_or("the pane is no longer attached")?;
        if !pane.input.is_unbound(&key) {
            return Err("a custom key binding owns Ctrl+V");
        }
        Ok(proto::ImagePasteTarget {
            daemon_id: destination.key.daemon.to_string(),
            pane_id: destination.key.pane.to_string(),
        })
    };
    let target = match target() {
        Ok(target) => target,
        Err(reason) => {
            log::debug("input.image.key_passed", fields! { "why" => reason });
            proto::ImagePasteTarget::default()
        }
    };
    Response { payload: Some(response::Payload::ImagePasteTarget(target)) }
}

pub(crate) fn prepare(window: WindowId, request: proto::PrepareImagePaste) -> Response {
    let named = (!request.pane_id.is_empty()).then(|| {
        PaneKey::new(
            &muster_core::composition::DaemonId::new(request.daemon_id),
            &muster_core::mirror::backend::PaneId::new(request.pane_id),
        )
    });
    let Some(destination) = session::image_destination(window, named) else {
        return Response::failure("the image's destination pane is gone; nothing was pasted");
    };
    if !destination.connected {
        return Response::failure("the image's daemon is disconnected; nothing was pasted");
    }
    // Reserve before native clipboard conversion, which runs outside the UI thread.
    let lease = match UploadBytes::take(MAX_IMAGE_BYTES) {
        Ok(lease) => lease,
        Err(why) => return Response::failure(why),
    };
    let key = destination.key.clone();
    let window = session::window_name(window);
    let status = Arc::new(Mutex::new(()));
    let beginning = poison::lock(&status, "image paste status");
    let finishing = status.clone();
    let finished_key = key.clone();
    let finished_window = window.clone();
    let upload = match destination.input.begin(&key.pane, move |result| {
        let _status = poison::lock(&finishing, "image paste status");
        let error = match result {
            Err(ImagePasteError::Failed(why)) => why,
            Ok(()) | Err(ImagePasteError::Canceled) => String::new(),
        };
        changed(&finished_window, &finished_key, false, error);
    }) {
        Ok(upload) => upload,
        Err(why) => return Response::failure(why),
    };
    changed(&window, &key, true, String::new());
    drop(beginning);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (deadline, completed) = mpsc::channel();
    poison::lock(&PREPARED, "prepared image pastes")
        .insert(id, Prepared { destination, upload, lease, deadline });
    let timer =
        std::thread::Builder::new().name("image conversion deadline".into()).spawn(move || {
            if completed.recv_timeout(Duration::from_secs(10))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                expire(id);
            }
        });
    if let Err(error) = timer {
        let pending = poison::lock(&PREPARED, "prepared image pastes").remove(&id);
        if let Some(pending) = pending {
            pending.destination.input.forget(&pending.destination.key.pane);
        }
        return Response::failure(format!(
            "could not start the image conversion deadline: {error}"
        ));
    }
    Response {
        payload: Some(response::Payload::ImagePastePrepared(proto::ImagePastePrepared {
            upload_id: id,
        })),
    }
}

pub(crate) fn paste(_window: WindowId, request: proto::PasteImage) -> Response {
    let Some(prepared) =
        poison::lock(&PREPARED, "prepared image pastes").remove(&request.upload_id)
    else {
        return Response::ok();
    };
    let Prepared { destination, upload, lease, deadline } = prepared;
    drop(deadline);
    if upload.cancellation.is_canceled() {
        return Response::ok();
    }
    let invalid =
        if request.error.is_empty() { check_png(&request.png).err() } else { Some(request.error) };
    if let Some(why) = invalid {
        destination.input.finish(&destination.key.pane, upload.id, Err(why));
        return Response::ok();
    }
    let key = destination.key.clone();
    let input = destination.input.clone();
    let worker_input = input.clone();
    let worker_key = key.clone();
    let id = upload.id;
    let bytes = request.png.len();
    let spawned = std::thread::Builder::new().name("image paste".into()).spawn(move || {
        let _lease = lease;
        let result = muster_daemon_client::images::store(
            Path::new(&destination.socket),
            &worker_key.pane,
            request.png,
            &upload.cancellation,
        );
        log::info(
            "input.image.finished",
            fields! {
                "daemon" => worker_key.daemon.to_string(),
                "pane" => worker_key.pane.to_string(),
                "bytes" => bytes,
                "stored" => result.is_ok(),
            },
        );
        worker_input.finish(&worker_key.pane, id, result);
    });
    if let Err(error) = spawned {
        let why = format!("could not start the image upload: {error}");
        input.finish(&key.pane, id, Err(why));
    }
    Response::ok()
}

fn expire(id: u64) {
    let pending = poison::lock(&PREPARED, "prepared image pastes").remove(&id);
    if let Some(pending) = pending {
        pending.destination.input.finish(
            &pending.destination.key.pane,
            pending.upload.id,
            Err("the clipboard image did not finish converting within ten seconds".into()),
        );
    }
}

fn changed(window: &str, key: &PaneKey, pasting: bool, error: String) {
    ffi::emit(
        &Event::new(event::Payload::ImagePasteChanged(proto::ImagePasteChanged {
            daemon_id: key.daemon.to_string(),
            pane_id: key.pane.to_string(),
            pasting,
            error,
        }))
        .for_window(window),
    );
}

pub(crate) fn forget() {
    let prepared = std::mem::take(&mut *poison::lock(&PREPARED, "prepared image pastes"));
    for (_, pending) in prepared {
        pending.destination.input.forget(&pending.destination.key.pane);
    }
}
