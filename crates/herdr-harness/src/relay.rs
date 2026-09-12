//! A real daemon behind a socket that loses some of its answers.
//!
//! What a loaded machine does to Muster's requests, on demand: herdr receives the request and
//! acts on it, and the answer does not come back in time. No request can ask a daemon to do
//! that, and waiting for a machine to be slow enough is a test that passes when it is not. So
//! this relays every connection to the daemon unchanged, except that for the methods it was
//! told about it reads herdr's answer and never delivers it.
//!
//! Not a hand-written herdr (`docs/testing.md`): every byte a caller receives is one the real
//! daemon sent, and the work behind a withheld answer is done by the real daemon. It stages a
//! transport fault, the same kind as the silent listener in `muster-herdr`'s subscription
//! tests.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

/// A socket in front of one daemon, withholding the answers to some methods.
///
/// Stops accepting on drop. Connections already relayed end when either side hangs up, which
/// for a test is when its daemon or its window goes.
#[derive(Debug)]
pub struct Relay {
    socket_path: PathBuf,
    config_path: PathBuf,
    running: Arc<AtomicBool>,
}

impl Relay {
    pub(crate) fn start(root: &Path, daemon: &Path, withheld: &[&str]) -> Relay {
        let socket_path = root.join("relay.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap_or_else(|error| {
            panic!("could not bind the relay at {}: {error}", socket_path.display())
        });
        let running = Arc::new(AtomicBool::new(true));
        let daemon = daemon.to_path_buf();
        let withheld: Vec<String> = withheld.iter().map(|method| (*method).to_string()).collect();
        let accepting = Arc::clone(&running);
        std::thread::spawn(move || {
            for client in listener.incoming() {
                if !accepting.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(client) = client else { continue };
                let daemon = daemon.clone();
                let withheld = withheld.clone();
                std::thread::spawn(move || relay(client, &daemon, &withheld));
            }
        });
        Relay { config_path: root.join("muster-relay.toml"), socket_path, running }
    }

    /// A Muster config naming this relay as the daemon `local`, which is how a window is
    /// pointed at it.
    pub fn muster_config(&self) -> PathBuf {
        let contents = format!(
            "[[daemon]]\nid = \"local\"\nsocket = {:?}\n",
            self.socket_path.to_string_lossy()
        );
        std::fs::write(&self.config_path, contents).unwrap_or_else(|error| {
            panic!(
                "could not write the relay's Muster config at {}: {error}",
                self.config_path.display()
            )
        });
        self.config_path.clone()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // The accept loop is parked in `accept`, and a connection is the one thing that wakes it
        // to read the flag.
        let _ = UnixStream::connect(&self.socket_path);
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn relay(mut client: UnixStream, daemon: &Path, withheld: &[String]) {
    let Some(request) = read_line(&mut client) else { return };
    let Ok(mut upstream) = UnixStream::connect(daemon) else { return };
    if upstream.write_all(&request).and_then(|()| upstream.write_all(b"\n")).is_err() {
        return;
    }

    let method = serde_json::from_slice::<Value>(&request)
        .ok()
        .and_then(|request| request.get("method").and_then(Value::as_str).map(str::to_string));
    if method.is_some_and(|method| withheld.contains(&method)) {
        // Read to the end of the answer, so the daemon has finished the work before this gives
        // the caller nothing.
        let _ = read_line(&mut upstream);
        // Held open rather than closed. A hang-up reaches the caller as an end of file, which is
        // a different failure from the silence under test.
        let _ = std::io::copy(&mut client, &mut std::io::sink());
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
