//! The app's way of talking to a pane's bridge process.
//!
//! The bridge exists because libghostty can only be fed by the command a surface spawns, so
//! the frame stream lives in a subprocess (`docs/observations/libghostty-9f9b8d1d.md`
//! section 2). That subprocess owns the pane's control stream, which is also the only
//! channel input can go out on - so the app needs a way to reach it.
//!
//! A socket rather than the surface's own PTY. Writing input through the surface would
//! widen the renderer seam from "run a pane channel into it" to "and also carry arbitrary
//! bytes back out", and would tie Muster's input path to a renderer it intends to be able
//! to replace.
//!
//! What crosses it towards the bridge is herdr's control-stream JSON, verbatim, so the bridge
//! stays a relay with no vocabulary of its own. What crosses it the other way is the bridge
//! speaking about itself, which is `bridge_report.rs`.
//!
//! **This is also how the app learns a pane's bridge has died**, and it is the only thing that
//! reliably does. The obvious candidate was libghostty's `close_surface` callback, and two
//! field runs on 0.4.1 show it never arriving: a dead pane sits on libghostty's own "Process
//! exited. Press any key" screen, which is the surface being held open rather than the host
//! being asked to close it, so nothing downstream of that callback has ever run (kan
//! a_2IRcMjFs0). This socket needs no such cooperation. The app opened it, the bridge dialed
//! it, and the connection ends when the process does - whether it exited, was killed, or lost
//! the machine it was running on.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::respawn::Ended;

use crate::bridge_report::Exiting;
use crate::control_stream::ControlStreamMessage;

/// Why a channel could not be opened.
#[derive(Debug)]
pub enum Failure {
    /// The path could not be bound - taken, too long, or in a directory we cannot write.
    BindFailed { path: String, detail: String },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::BindFailed { path, detail } => {
                write!(f, "could not bind {path} ({detail})")
            }
        }
    }
}

impl std::error::Error for Failure {}

/// What a channel tells its owner about the bridge on the other end.
///
/// A struct rather than two arguments, because both are closures of nearly the same shape and
/// a caller that swapped them would compile. The same trap cost `View::of` its fourth closure
/// (kan a_2HrmSyRAQ), and named fields are what stop it happening twice.
pub struct Reports {
    /// A bridge dialed in. Runs on the accepting thread, each time - a pane keeps its channel
    /// while its surface is thrown away and built again, so a replacement dials too.
    pub connected: Box<dyn Fn() + Send + Sync>,

    /// The bridge that was on the other end has stopped. Runs on that connection's own reader
    /// thread, at most once per connection.
    pub exited: Box<dyn Fn(Ended) + Send + Sync>,
}

impl std::fmt::Debug for Reports {
    /// Closures have nothing to say about themselves, and a channel's `Debug` should not stop
    /// existing because it holds two.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reports { connected, exited }")
    }
}

#[derive(Debug)]
pub struct PaneControlChannel {
    path: String,
    /// Set once the bridge dials in. `None` is normal exactly once - between the surface
    /// starting and the bridge connecting - and a real problem after that.
    client: Arc<Mutex<Option<UnixStream>>>,
    /// Told to the accepting thread by `drop`, and read by it after every accept.
    closing: Arc<AtomicBool>,
    /// Set by the accepting thread on its way out.
    stopped: Arc<AtomicBool>,
}

impl PaneControlChannel {
    /// Opens a channel and starts waiting for the bridge.
    ///
    /// The socket is bound before this returns, and so before the surface is created, which
    /// is what stops the bridge losing a race against its own listener.
    ///
    /// `reports.connected` runs on the accepting thread each time a bridge dials in. It exists
    /// because that fact is the single most useful one in the log when keystrokes go nowhere,
    /// and because it is the shell's first reason to be told something it did not ask about.
    ///
    /// Each time, not once: a pane keeps its channel while its surface is thrown away and
    /// built again, which happens whenever a window has to rebuild a pane. A listener that
    /// accepted once would leave the replacement bridge unable to connect at all - and that
    /// pane would render, paint, and swallow every keystroke, which is the exact failure this
    /// whole path exists to prevent.
    pub fn bind(path: impl Into<String>, reports: Reports) -> Result<PaneControlChannel, Failure> {
        let path = path.into();
        // A path left behind by a crashed run would make bind fail with EADDRINUSE; nothing
        // else can legitimately own this path, since it carries our own pid.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).map_err(|error| Failure::BindFailed {
            path: path.clone(),
            detail: error.to_string(),
        })?;

        log::info("channel.listening", fields! { "path" => &path });

        let client = Arc::new(Mutex::new(None));
        let accepting = Arc::clone(&client);
        let accept_path = path.clone();
        let closing = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        // Which connection is the live one, counting from one. A reader compares this against
        // its own number before reporting: without it, replacing a bridge would report the one
        // it replaced as having died, which is true, useless, and would have the core count a
        // replacement against a pane that had just been given one.
        let counting = Arc::new(AtomicU64::new(0));
        let reports = Arc::new(reports);
        let (told, telling) = (Arc::clone(&closing), Arc::clone(&stopped));
        std::thread::spawn(move || {
            let mut connections = 0u64;
            loop {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) => {
                        // Nothing retries after this, so the pane is typed-into-the-void from
                        // here on.
                        log::error(
                            "channel.accept.failed",
                            fields! {
                                "path" => &accept_path,
                                "detail" => error.to_string(),
                                "impact" => "this pane will never become typeable; it keeps \
                                             rendering, so it looks frozen rather than broken",
                            },
                        );
                        telling.store(true, Ordering::Release);
                        return;
                    }
                };
                // The channel is going away and this connection is its own doing. Checked
                // after the accept rather than before, because a thread parked in `accept` is
                // not checking anything - which is why the drop has to knock first.
                if told.load(Ordering::Acquire) {
                    telling.store(true, Ordering::Release);
                    return;
                }

                // Without this, writing to a bridge that has died raises SIGPIPE and kills the
                // app - one pane's subprocess crashing would take every other pane's window
                // with it.
                silence_sigpipe(&stream);

                // A second descriptor on the same socket, for the thread that waits for this
                // bridge to stop. Reads and writes on a socket are independent, so it does not
                // contend with `send`, and a clone rather than a shared lock means a reader
                // parked on `read` is never holding anything the input path wants.
                let watching = stream.try_clone().ok();

                connections += 1;
                counting.store(connections, Ordering::Release);

                // The newest bridge wins, because the one it replaced belongs to a surface
                // that has already been thrown away. Shutting the old connection down is how
                // that bridge learns to exit - said outright rather than left to the drop,
                // now that a reader holds a descriptor of its own and dropping one end would
                // no longer close the socket.
                let replaced = poison::lock(&accepting, "pane-control-channel").replace(stream);
                if let Some(old) = replaced {
                    let _ = old.shutdown(Shutdown::Both);
                }
                log::info(
                    "channel.connected",
                    fields! { "path" => &accept_path, "connection" => connections.to_string() },
                );
                if let Some(watching) = watching {
                    watch(watching, connections, &counting, &told, &reports, &accept_path);
                }
                (reports.connected)();
            }
        });

        Ok(PaneControlChannel { path, client, closing, stopped })
    }

    /// The path to hand the bridge.
    pub fn socket_path(&self) -> &str {
        &self.path
    }

    /// Whether the accepting thread has finished, for a test that needs to prove it does.
    ///
    /// A thread parked on a listener nobody will dial is invisible from outside the process
    /// and unobservable through any other API, and it is exactly what a closed pane would
    /// leave behind - one per pane, for as long as the window is open. Handed out as a flag
    /// rather than a join handle because joining inside `drop` would turn a wake-up that did
    /// not arrive into a hang on quit, which is a worse failure than the leak.
    pub fn stopped(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }

    /// Sends a message to the pane, if the bridge has connected.
    ///
    /// Returns whether it went out. A false is normal exactly once - in the moment between
    /// the surface starting and the bridge dialing back - and is a real problem after that,
    /// which is why the caller is told rather than the failure being swallowed here.
    pub fn send(&self, message: &ControlStreamMessage) -> bool {
        let mut slot = poison::lock(&self.client, "pane-control-channel");
        let Some(stream) = slot.as_mut() else { return false };
        match stream.write_all(&message.wire_format()) {
            Ok(()) => true,
            Err(error) => {
                log::error(
                    "channel.send.failed",
                    fields! {
                        "detail" => error.to_string(),
                        "impact" => "this input reached nothing; the bridge is gone or its \
                                     socket closed",
                    },
                );
                false
            }
        }
    }
}

impl Drop for PaneControlChannel {
    fn drop(&mut self) {
        // Knock, then take the door away. The accepting thread is parked inside `accept` and
        // nothing else will wake it: a pane that closes would otherwise leave a thread there
        // for as long as the window is open, one per pane, and a window whose panes come and
        // go all day would collect them silently. The connection is thrown away by the thread
        // the moment it reads the flag this sets.
        self.closing.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
        // And wake the reader, which is parked on a descriptor of its own that dropping the
        // slot below would no longer close. It reads the flag above and reports nothing: this
        // bridge is ending because the pane it belonged to is gone.
        if let Some(stream) = poison::lock(&self.client, "pane-control-channel").as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Waits for one bridge to stop, and says so once.
///
/// The whole of Muster's own answer to "is this pane's bridge still alive". A bridge that
/// exits, is killed, or loses the machine it was running on all end the same way from here,
/// because what is being watched is the connection rather than the process - and unlike
/// libghostty's `close_surface`, this arrives.
///
/// Silent in two cases, and they are the two that would otherwise make this harmful. A
/// connection that is no longer the live one belongs to a bridge that has already been
/// replaced, so reporting it would count a replacement against a pane that has just been
/// given one. A channel that is closing belongs to a pane that has gone, and its bridge is
/// ending because Muster ended it.
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
        let mut lines = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            match lines.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if let Some(exiting) = Exiting::parse(line.strip_suffix(b"\n").unwrap_or(&line)) {
                said = Some(exiting);
            }
        }

        if closing.load(Ordering::Acquire) || current.load(Ordering::Acquire) != generation {
            return;
        }
        let ended = said.map_or_else(Ended::unsaid, |exiting| Ended {
            ending: exiting.ending,
            reason: exiting.reason,
            rendered: exiting.rendered,
        });
        log::info(
            "channel.bridge.gone",
            fields! {
                "path" => &path,
                "connection" => generation.to_string(),
                "ending" => ended.ending.as_str(),
                "reason" => ended.reason.clone().unwrap_or_else(|| "(it said nothing)".into()),
                "rendered" => ended.rendered.to_string(),
            },
        );
        (reports.exited)(ended);
    });
}

/// Stops a write to a peer that has gone from killing this process.
///
/// Public because both ends of this socket need it and for the same reason: one pane's
/// subprocess dying must not take the window with it, and a bridge must not be killed
/// mid-sentence while reporting why it is stopping. macOS spells it as a socket option rather
/// than a per-write flag, which is why it is a function rather than a call site.
pub fn silence_sigpipe(stream: &UnixStream) {
    let on: libc::c_int = 1;
    // SAFETY: the fd is owned by `stream` and outlives the call; the option value is an
    // int of the size reported.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            std::ptr::from_ref(&on).cast(),
            u32::try_from(size_of::<libc::c_int>()).expect("an int fits a socklen"),
        );
    }
}
