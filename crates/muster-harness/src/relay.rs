//! A real daemon behind a socket that loses or delays some of its answers.
//!
//! What a loaded machine does to Muster's requests, on demand: the daemon receives the request
//! and acts on it, and the answer does not come back in time. No request can ask a daemon to do
//! that, and waiting for a machine to be slow enough is a test that passes when it is not. So a
//! relay passes every connection through to the daemon unchanged, except that for the requests
//! it was told about it reads the daemon's answer and either never delivers it or delivers it
//! late.
//!
//! Not a hand-written daemon (`docs/testing.md`): every byte a caller receives is one the real
//! daemon sent, and the work behind a withheld answer is done by the real daemon. It stages a
//! transport fault.
//!
//! This is the part that does not depend on which daemon is behind it: the socket, the accept
//! loop, and the config naming it. A [`Pump`] carries each connection, and knows the wire well
//! enough to find a request and its answer - JSON lines for herdr (`herdr-harness`), framed
//! protobuf for muster-daemon.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long a relay keeps an answer to one of the requests it was told about.
#[derive(Debug, Clone, Copy)]
pub enum Holding {
    /// Never delivered: a lost answer.
    Forever,
    /// Delivered this long after the daemon gave it: a late one.
    For(Duration),
}

/// Carries one connection between a caller and the daemon, withholding what it was told to.
pub trait Pump: Send + Sync + 'static {
    /// Returns when either side has gone.
    fn carry(&self, client: UnixStream, daemon: &Path);
}

/// A socket in front of one daemon, withholding or delaying the answers to some requests.
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
    /// Listens in `root`, beside the daemon it relays to, so both go when the test's root does.
    pub fn start(root: &Path, daemon: &Path, pump: Arc<dyn Pump>) -> Relay {
        let socket_path = root.join("relay.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap_or_else(|error| {
            panic!("could not bind the relay at {}: {error}", socket_path.display())
        });
        let running = Arc::new(AtomicBool::new(true));
        let daemon = daemon.to_path_buf();
        let accepting = Arc::clone(&running);
        std::thread::spawn(move || {
            for client in listener.incoming() {
                if !accepting.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(client) = client else { continue };
                let daemon = daemon.clone();
                let pump = Arc::clone(&pump);
                std::thread::spawn(move || pump.carry(client, &daemon));
            }
        });
        Relay { config_path: root.join("muster-relay.toml"), socket_path, running }
    }

    /// Where a caller dials to reach the daemon through the relay.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
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
