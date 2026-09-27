//! muster-daemon's half of the answer-withholding relay: framed protobuf, one connection
//! carrying many requests, and events arriving between answers.
//!
//! A withheld answer is dropped or held while everything else on the connection passes, so a
//! test sees what a lost answer really looks like here: the events the request produced arrive,
//! and the answer naming them does not.

use std::collections::HashSet;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use muster_daemon_proto::connection::LARGEST_MESSAGE;
use muster_daemon_proto::{self as proto, ConnectionKind, control_message};
use muster_frame::{read_frame_or_end, write_frame};
use prost::Message;

use crate::relay::{Holding, Pump};

pub(crate) type Withheld = Arc<dyn Fn(&proto::Request) -> bool + Send + Sync>;

pub(crate) struct DaemonPump {
    withheld: Withheld,
    holding: Holding,
}

impl DaemonPump {
    pub(crate) fn new(withheld: Withheld, holding: Holding) -> DaemonPump {
        DaemonPump { withheld, holding }
    }
}

impl Pump for DaemonPump {
    fn carry(&self, client: UnixStream, daemon: &Path) {
        let Ok(upstream) = UnixStream::connect(daemon) else { return };
        let (Ok(mut from_client), Ok(mut to_daemon)) = (client.try_clone(), upstream.try_clone())
        else {
            return;
        };
        let (mut to_client, mut from_daemon) = (client, upstream);
        let ids: Arc<Mutex<HashSet<u64>>> = Arc::default();

        let withheld = Arc::clone(&self.withheld);
        let noting = Arc::clone(&ids);
        std::thread::spawn(move || {
            let mut control = None;
            while let Ok(Some(frame)) = read_frame_or_end(&mut from_client, LARGEST_MESSAGE) {
                match control {
                    None => {
                        control = Some(
                            proto::Hello::decode(frame.as_slice())
                                .is_ok_and(|hello| hello.kind() == ConnectionKind::Control),
                        );
                    }
                    Some(true) => {
                        // Noted before it is passed on, so it is known before any answer can be.
                        if let Ok(request) = proto::Request::decode(frame.as_slice())
                            && withheld(&request)
                        {
                            noting.lock().expect("the relay's id set").insert(request.id);
                        }
                    }
                    Some(false) => {}
                }
                if write_frame(&mut to_daemon, &frame).is_err() {
                    break;
                }
            }
            let _ = to_daemon.shutdown(Shutdown::Both);
        });

        let mut welcomed = false;
        while let Ok(Some(frame)) = read_frame_or_end(&mut from_daemon, LARGEST_MESSAGE) {
            if welcomed && withholds(&frame, &ids) {
                match self.holding {
                    Holding::Forever => continue,
                    Holding::For(delay) => std::thread::sleep(delay),
                }
            }
            welcomed = true;
            if write_frame(&mut to_client, &frame).is_err() {
                break;
            }
        }
        let _ = to_client.shutdown(Shutdown::Both);
    }
}

/// Whether a frame from the daemon is the answer to a request being withheld.
fn withholds(frame: &[u8], ids: &Mutex<HashSet<u64>>) -> bool {
    let Ok(message) = proto::ControlMessage::decode(frame) else { return false };
    let Some(control_message::Message::Answer(answer)) = message.message else { return false };
    ids.lock().expect("the relay's id set").remove(&answer.id)
}
