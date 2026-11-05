use std::net::Shutdown;
use std::path::Path;
use std::time::Duration;

use muster_core::input::ImageCancellation;
use muster_core::input::image::check_png;
use muster_core::mirror::backend::PaneId;
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, answer, control_message, pane_request, request};

const PATIENCE: Duration = Duration::from_secs(10);

pub fn store(
    socket: &Path,
    pane: &PaneId,
    png: Vec<u8>,
    cancellation: &ImageCancellation,
) -> Result<String, String> {
    check_png(&png)?;
    if cancellation.is_canceled() {
        return Err("the image paste was canceled".into());
    }
    let (mut stream, welcome) =
        crate::dial(socket, proto::ConnectionKind::Control, "muster image paste")
            .map_err(|error| error.to_string())?;
    if let Some(why) = proto::version::too_old(
        "the pane's daemon",
        welcome.protocol.as_ref(),
        proto::version::IMAGE_PASTE,
        "store pasted images",
        "pasted",
    ) {
        return Err(why);
    }
    let stop = stream.try_clone().map_err(|error| error.to_string())?;
    cancellation.on_cancel(move || {
        let _ = stop.shutdown(Shutdown::Both);
    });
    stream.set_write_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(PATIENCE)).map_err(|error| error.to_string())?;
    let request = proto::Request {
        id: 1,
        service: Some(request::Service::Pane(proto::PaneRequest {
            request: Some(pane_request::Request::StoreImage(pane_request::StoreImage {
                pane: pane.to_string(),
                png,
            })),
        })),
    };
    connection::send(&mut stream, &request)
        .map_err(|error| format!("the image upload stopped making progress: {error}"))?;
    loop {
        let message = connection::receive::<proto::ControlMessage>(&mut stream)
            .map_err(|error| format!("the image upload was not answered: {error}"))?
            .ok_or("the daemon disconnected before confirming the image upload")?;
        if let Some(control_message::Message::Answer(answer)) = message.message {
            if answer.id != 1 {
                return Err("the daemon answered a different image upload".into());
            }
            if answer.outcome() != proto::Outcome::Done {
                return Err(answer.reason);
            }
            return match answer.detail {
                Some(answer::Detail::ImageStored(image)) if !image.path.is_empty() => {
                    Ok(image.path)
                }
                _ => Err("the daemon confirmed an image upload without its file path".into()),
            };
        }
    }
}
