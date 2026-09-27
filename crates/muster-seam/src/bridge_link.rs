//! The socket a pane's bridge reports on, and the window's end of it.
//!
//! The bridge dials it before it attaches to its pane's stream, so an attach the daemon refuses
//! can still say why, and holds it for its whole life, so the connection ending is how the window
//! learns a bridge died - whether it exited, was killed, or lost the machine it was running on.
//! libghostty's `close_surface` would be the obvious signal, and two field runs showed it never
//! arriving: a dead pane sits on libghostty's own "Process exited" screen, which is the surface
//! held open rather than the host asked to close it (kan a_2IRcMjFs0). What travels on it is
//! `muster_core::bridge_link`.

use std::io::{BufRead, BufReader};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use muster_core::bridge_link::Report;
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::respawn::Ended;

/// What a link tells its owner about the bridge on the other end.
///
/// A struct rather than positional arguments, because the closures are nearly the same shape
/// and a caller that swapped two would compile (kan a_2HrmSyRAQ).
pub(crate) struct Reports {
    /// A bridge attached to the pane. Runs on that connection's reader thread, each time: a
    /// pane keeps its link while its surface is thrown away and built again, so a replacement
    /// attaches too.
    pub(crate) attached: Box<dyn Fn() + Send + Sync>,
    /// The bridge has stopped. Runs on its connection's reader thread, at most once.
    pub(crate) exited: Box<dyn Fn(Ended) + Send + Sync>,
    /// The bridge painted. At most four times a second while output is arriving
    /// (`muster_core::painting`).
    pub(crate) painted: Box<dyn Fn() + Send + Sync>,
}

/// One pane's link, bound and listening.
#[derive(Debug)]
pub(crate) struct PaneLink {
    path: String,
    /// The live bridge's connection, kept to shut it down when a newer bridge dials.
    client: Arc<Mutex<Option<UnixStream>>>,
    /// Told to the accepting thread by `drop`, and read by it after every accept.
    closing: Arc<AtomicBool>,
}

impl PaneLink {
    /// Binds the socket and starts waiting for a bridge.
    ///
    /// Bound before this returns, and so before the surface is created, which is what stops the
    /// bridge losing a race against its own listener.
    pub(crate) fn bind(path: impl Into<String>, reports: Reports) -> Result<PaneLink, String> {
        let path = path.into();
        // A path left behind by a crashed run would fail the bind; nothing else can own this
        // path, since it carries our own pid.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .map_err(|error| format!("could not bind {path} ({error})"))?;
        log::info("link.listening", fields! { "path" => &path });

        let client = Arc::new(Mutex::new(None));
        let closing = Arc::new(AtomicBool::new(false));
        // Which connection is the live one. A reader compares this against its own number before
        // reporting an end: without it, replacing a bridge would report the one it replaced as
        // having died, and the core would count a replacement against a pane just given one.
        let current = Arc::new(AtomicU64::new(0));
        let reports = Arc::new(reports);
        let (accepting, told, accept_path) =
            (Arc::clone(&client), Arc::clone(&closing), path.clone());
        std::thread::spawn(move || {
            let mut connections = 0u64;
            loop {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) => {
                        log::error(
                            "link.accept.failed",
                            fields! {
                                "path" => &accept_path,
                                "detail" => error.to_string(),
                                "impact" => "no bridge can report on this pane from now on, so \
                                             the window counts it as waiting for one and starts \
                                             replacements it cannot hear from",
                            },
                        );
                        return;
                    }
                };
                // The link is going away and this connection is its own doing: `drop` knocks
                // to wake a thread parked in `accept`.
                if told.load(Ordering::Acquire) {
                    return;
                }
                let watching = stream.try_clone().ok();
                connections += 1;
                current.store(connections, Ordering::Release);
                // The newest bridge wins, because the one it replaced belongs to a surface
                // already thrown away.
                if let Some(old) = poison::lock(&accepting, "pane-link").replace(stream) {
                    let _ = old.shutdown(Shutdown::Both);
                }
                log::info(
                    "link.connected",
                    fields! { "path" => &accept_path, "connection" => connections },
                );
                if let Some(watching) = watching {
                    watch(watching, connections, &current, &told, &reports, &accept_path);
                }
            }
        });

        Ok(PaneLink { path, client, closing })
    }

    /// The path to hand the bridge.
    pub(crate) fn socket_path(&self) -> &str {
        &self.path
    }
}

impl Drop for PaneLink {
    fn drop(&mut self) {
        // Knock, then take the door away: nothing else wakes a thread parked in `accept`, and a
        // window whose panes come and go all day would collect them.
        self.closing.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
        // And wake the reader, which reads the flag and reports nothing: this bridge is ending
        // because its pane is gone.
        if let Some(stream) = poison::lock(&self.client, "pane-link").as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Reads one bridge's reports until it stops, and says it stopped once.
///
/// Silent about the end in two cases: a connection that is no longer the live one belongs to a
/// bridge already replaced, and a link that is closing belongs to a pane that has gone.
fn watch(
    stream: UnixStream,
    generation: u64,
    current: &Arc<AtomicU64>,
    closing: &Arc<AtomicBool>,
    reports: &Arc<Reports>,
    path: &str,
) {
    let (current, closing, reports, path) =
        (Arc::clone(current), Arc::clone(closing), Arc::clone(reports), path.to_string());
    std::thread::spawn(move || {
        let mut said = None;
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            match Report::parse(&line) {
                Some(Report::Attached) => (reports.attached)(),
                Some(Report::Painted { .. }) => (reports.painted)(),
                Some(Report::Exiting(ended)) => said = Some(ended),
                None => log::debug("link.unread", fields! { "path" => &path, "line" => line }),
            }
        }
        if closing.load(Ordering::Acquire) || current.load(Ordering::Acquire) != generation {
            return;
        }
        let ended = said.unwrap_or_else(Ended::unsaid);
        log::info(
            "link.bridge.gone",
            fields! {
                "path" => &path,
                "connection" => generation,
                "ending" => ended.ending.as_str(),
                "reason" => ended.reason.clone().unwrap_or_else(|| "(it said nothing)".into()),
                "rendered" => ended.rendered,
            },
        );
        (reports.exited)(ended);
    });
}
