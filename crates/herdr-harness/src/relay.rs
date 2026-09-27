//! herdr's half of the answer-withholding relay: which request on a connection is the one to
//! withhold, and where its answer ends. The socket and the accept loop are
//! `muster_harness::Relay`'s.
//!
//! herdr answers one newline-terminated JSON request per connection and then hangs up, so the
//! request is the first line a caller sends and the answer is the first line herdr sends back.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use muster_harness::{Holding, Pump};
use serde_json::Value;

/// Whether a relay withholds the answer to a request, asked of each request as herdr reads it.
pub(crate) type Withheld = Arc<dyn Fn(&Value) -> bool + Send + Sync>;

/// Carries herdr's JSON lines, withholding the answers `withheld` picks out.
pub(crate) struct HerdrPump {
    pub(crate) withheld: Withheld,
    pub(crate) holding: Holding,
}

impl Pump for HerdrPump {
    fn carry(&self, client: UnixStream, daemon: &Path) {
        relay(client, daemon, &self.withheld, self.holding);
    }
}

fn relay(mut client: UnixStream, daemon: &Path, withheld: &Withheld, holding: Holding) {
    let Some(request) = read_line(&mut client) else { return };
    let Ok(mut upstream) = UnixStream::connect(daemon) else { return };
    if upstream.write_all(&request).and_then(|()| upstream.write_all(b"\n")).is_err() {
        return;
    }

    if serde_json::from_slice::<Value>(&request).is_ok_and(|request| withheld(&request)) {
        // Read to the end of the answer, so the daemon has finished the work before this gives
        // the caller nothing.
        let answer = read_line(&mut upstream);
        match holding {
            // Held open rather than closed. A hang-up reaches the caller as an end of file,
            // which is a different failure from the silence under test.
            Holding::Forever => {
                let _ = std::io::copy(&mut client, &mut std::io::sink());
            }
            // Then hung up, as herdr does once it has answered.
            Holding::For(delay) => {
                std::thread::sleep(delay);
                if let Some(answer) = answer {
                    let _ = client.write_all(&answer).and_then(|()| client.write_all(b"\n"));
                }
                let _ = client.shutdown(Shutdown::Both);
            }
        }
        return;
    }

    let (Ok(mut from_client), Ok(mut to_client)) = (client.try_clone(), client.try_clone()) else {
        return;
    };
    let Ok(mut to_daemon) = upstream.try_clone() else { return };
    // The daemon's side is closed only once the caller has gone altogether, never half-closed
    // while it waits: herdr reads a half-closed caller as one that has gone, and a relay that did
    // that would change the behaviour it is supposed to pass through.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_client, &mut to_daemon);
        let _ = to_daemon.shutdown(Shutdown::Both);
    });
    let _ = std::io::copy(&mut upstream, &mut to_client);
    let _ = client.shutdown(Shutdown::Both);
}

/// One newline-terminated line, without the newline, read a byte at a time so nothing after it
/// is taken from the stream.
fn read_line(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => return Some(line),
            Ok(1) => line.push(byte[0]),
            _ => return None,
        }
    }
}
