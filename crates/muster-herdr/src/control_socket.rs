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
//! What crosses it is herdr's control-stream JSON, verbatim, so the bridge stays a relay
//! with no vocabulary of its own.

use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;

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
    /// `on_connect` runs on the accepting thread each time a bridge dials in. It exists
    /// because that fact is the single most useful one in the log when keystrokes go nowhere,
    /// and because it is the shell's first reason to be told something it did not ask about.
    ///
    /// Each time, not once: a pane keeps its channel while its surface is thrown away and
    /// built again, which happens whenever a window has to rebuild a pane. A listener that
    /// accepted once would leave the replacement bridge unable to connect at all - and that
    /// pane would render, paint, and swallow every keystroke, which is the exact failure this
    /// whole path exists to prevent.
    pub fn bind(
        path: impl Into<String>,
        on_connect: impl Fn() + Send + 'static,
    ) -> Result<PaneControlChannel, Failure> {
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
                // with it. macOS spells it as a socket option rather than a per-write flag.
                set_nosigpipe(&stream);

                // The newest bridge wins, because the one it replaced belongs to a surface
                // that has already been thrown away. Dropping the old stream here closes it,
                // which is how that bridge learns to exit.
                *poison::lock(&accepting, "pane-control-channel") = Some(stream);
                log::info(
                    "channel.connected",
                    fields! { "path" => &accept_path, "connection" => connections.to_string() },
                );
                connections += 1;
                on_connect();
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
    }
}

fn set_nosigpipe(stream: &UnixStream) {
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
