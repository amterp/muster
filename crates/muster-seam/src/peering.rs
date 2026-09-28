//! Links this machine's daemon to each other machine's, so a group of messages can span them
//! (MIP-4, section 11). The window holds each link, as MIP-4's decision 4a has it: it tells the
//! daemon here where the other machine's daemon is forwarded, with a `msg.peer` request it keeps
//! open, and the daemon links for as long as the request lasts. So messages cross machines only
//! while a window runs, and a window that quits ends its links by hanging up.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto as proto;
use muster_daemon_proto::connection;
use proto::{ConnectionKind, control_message};

/// How often a holder looks up to see whether it should stop, while it waits to ask again.
const LOOK_UP: Duration = Duration::from_millis(250);

/// How long a holder waits before asking again, once the daemon here has hung up or refused.
const AGAIN_AT_MOST: Duration = Duration::from_secs(5);

/// A `msg.peer` request held open on this machine's daemon. Dropping it hangs up.
#[derive(Debug)]
pub(crate) struct Held {
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl Held {
    /// Holds, on the daemon at `here`, a link to the machine the window calls `name`, whose
    /// daemon is forwarded to `there`. Asked again whenever the daemon here goes away, since a
    /// daemon that restarts or hands its panes over forgets the request with the connection.
    pub(crate) fn start(here: PathBuf, name: String, there: PathBuf) -> Held {
        let held = Held { stop: Arc::new(AtomicBool::new(false)), stream: Arc::default() };
        let (stop, stream) = (Arc::clone(&held.stop), Arc::clone(&held.stream));
        let started = std::thread::Builder::new()
            .name(format!("peer-{name}"))
            .spawn(move || hold(&here, &name, &there, &stop, &stream));
        if let Err(error) = started {
            log::error(
                "peer.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "no group of messages spans this machine and that one while \
                                 this window runs",
                    "check" => "whether the app is out of threads",
                },
            );
        }
        held
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = poison::lock(&self.stream, "peer.stream").take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn hold(
    here: &Path,
    name: &str,
    there: &Path,
    stop: &AtomicBool,
    slot: &Mutex<Option<UnixStream>>,
) {
    let mut wait = LOOK_UP;
    while !stop.load(Ordering::Acquire) {
        let why = ask(here, name, there, slot).unwrap_or_else(|why| why);
        if stop.load(Ordering::Acquire) {
            break;
        }
        log::info(
            "peer.lost",
            fields! { "machine" => name, "daemon" => here.display().to_string(), "why" => why },
        );
        let mut slept = Duration::ZERO;
        while slept < wait && !stop.load(Ordering::Acquire) {
            std::thread::sleep(LOOK_UP);
            slept += LOOK_UP;
        }
        wait = (wait * 2).min(AGAIN_AT_MOST);
    }
}

/// Sends the request and holds it until the daemon answers it, which it does only to refuse,
/// or hangs up. Says which, and why.
fn ask(
    here: &Path,
    name: &str,
    there: &Path,
    slot: &Mutex<Option<UnixStream>>,
) -> Result<String, String> {
    let client = format!("muster {} peer", env!("CARGO_PKG_VERSION"));
    let (mut stream, _) = connection::connect(here, ConnectionKind::Control, &client)
        .map_err(|error| error.to_string())?;
    *poison::lock(slot, "peer.stream") =
        Some(stream.try_clone().map_err(|error| error.to_string())?);
    let peer = proto::msg_request::Peer {
        name: name.to_string(),
        socket: there.to_string_lossy().into_owned(),
    };
    let request = proto::Request {
        id: 1,
        service: Some(proto::request::Service::Msg(proto::MsgRequest {
            caller: None,
            request: Some(proto::msg_request::Request::Peer(peer)),
        })),
    };
    connection::send(&mut stream, &request).map_err(|error| error.to_string())?;
    log::info("peer.held", fields! { "machine" => name, "daemon" => here.display().to_string() });
    loop {
        match connection::receive::<proto::ControlMessage>(&mut stream)? {
            None => return Ok("the daemon hung up".to_string()),
            Some(proto::ControlMessage {
                message: Some(control_message::Message::Answer(answer)),
            }) => {
                if answer.outcome() == proto::Outcome::Refused {
                    log::warn(
                        "peer.refused",
                        fields! {
                            "machine" => name,
                            "reason" => answer.reason,
                            "impact" => "no group of messages spans this machine and that one \
                                         while this window runs; the window asks again",
                            "check" => "whether this machine's daemon is older than the app \
                                        (it knows no `msg.peer`), or the [[daemon]] id cannot \
                                        name a machine: letters, digits, '.', '_' and '-'",
                        },
                    );
                }
                return Ok(format!("the daemon answered {:?}", answer.outcome()));
            }
            Some(_) => {}
        }
    }
}
