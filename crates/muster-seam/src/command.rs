//! Requests from outside this process, answered exactly like requests from inside it.
//!
//! Muster has one action path: every chord and menu item becomes a `Request` and goes through
//! [`dispatch`] (architecture.md, one action path). That path was always meant to serve a CLI
//! too, and this is the whole of it - the same schema, the same dispatcher, arriving on a unix
//! socket instead of over the C ABI. Nothing here decides anything, which is the point: a
//! second entry point that made its own decisions would be a second Muster.
//!
//! It has to be Muster's own socket rather than the daemon's. A daemon knows its panes and
//! nothing about regions, tabs, focus, the arrangement, or the other daemon this same window
//! is showing - so an agent talking to herdr can make a pane behind Muster's back and cannot
//! ask what happened to it.
//!
//! One request per connection. A caller runs one command and exits, so a session would be
//! state to keep on both sides for no one's benefit, and framing a stream nobody reuses is
//! work that buys nothing. A thread each, because an answer can take a while - a pane being
//! created waits on a daemon, and a caller waiting for that must not hold up a caller asking
//! what the window looks like.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;

use crate::dispatch;

/// The endpoint this process is listening on, held so that it stays open.
///
/// A static because there is one window per process and it listens for the whole run. Replaced
/// rather than added to: a second startup in one process is a test reusing it, and the old
/// socket has to be given up before the new one can be bound.
static LISTENING: Mutex<Option<CommandEndpoint>> = Mutex::new(None);

/// Starts listening, or stops if the path is empty.
///
/// Failing to bind is a warning and not a refusal. The window works without an endpoint - it
/// just cannot be driven from outside - and refusing startup over it would turn "somebody left
/// a file in ~/.muster/state" into an app that will not open.
pub fn listen(path: &str) {
    let mut held = poison::lock(&LISTENING, "command-endpoint");
    // Given up before the new one is bound, because dropping an endpoint unlinks its path. The
    // other order works until somebody rebinds the same path, and then the old endpoint's drop
    // deletes the socket file the new one is listening on - leaving a listener nobody can dial
    // and no way to tell from in here.
    *held = None;

    if path.is_empty() {
        return;
    }
    *held = match CommandEndpoint::bind(path) {
        Ok(endpoint) => Some(endpoint),
        Err(failure) => {
            log::warn(
                "command.listen.failed",
                fields! {
                    "detail" => failure.to_string(),
                    "impact" => "nothing outside this process can drive this window - no CLI, no \
                                 script, and no agent running in one of its panes. The window \
                                 itself works normally, so this looks like the CLI being broken",
                    "check" => "whether that directory exists and is writable, and whether a \
                                file is already there from a run that was killed",
                },
            );
            None
        }
    };
}

/// Where this process is listening, if it is.
///
/// What a pane on this machine is told, so a program inside it can reach the window it is drawn
/// in. `None` when there is no endpoint or binding one failed - a pane is then told nothing
/// rather than a path nobody answers, because a caller that dials and is refused cannot tell
/// that from a Muster that has quit.
pub fn listening_at() -> Option<String> {
    poison::lock(&LISTENING, "command-endpoint")
        .as_ref()
        .map(|endpoint| endpoint.socket_path().to_string())
}

/// The most a message either way may be.
///
/// Every request Muster has is a few hundred bytes and the largest imaginable is a paste; the
/// largest answer is a `Window` for a window nobody can fill past about fifteen panes. This is
/// here so that a caller who is not Muster's CLI - a port scanner, a truncated write, a client
/// built against a different schema - cannot make the app reserve a gigabyte by claiming to be
/// about to send one, and so that the CLI is protected the same way from the same mistake.
pub const LARGEST_MESSAGE: u32 = 1 << 20;

/// How long a caller has to send its request, and to take its answer.
///
/// A connection that opens and says nothing would otherwise hold a thread for as long as the
/// window is open. Generous, because the deadline is against a stalled peer rather than a slow
/// one, and every legitimate caller has its bytes ready before it dials.
const PATIENCE: Duration = Duration::from_secs(30);

/// Why the endpoint could not be opened.
#[derive(Debug)]
pub enum Failure {
    BindFailed { path: String, detail: String },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::BindFailed { path, detail } => write!(f, "could not bind {path} ({detail})"),
        }
    }
}

impl std::error::Error for Failure {}

/// A listening socket, alive for as long as this is held.
#[derive(Debug)]
pub struct CommandEndpoint {
    path: String,
    /// Told to the accepting thread by `drop`, and read by it after every accept.
    closing: Arc<AtomicBool>,
    /// Set by the accepting thread on its way out.
    stopped: Arc<AtomicBool>,
}

impl CommandEndpoint {
    /// Starts answering requests on `path`.
    pub fn bind(path: impl Into<String>) -> Result<CommandEndpoint, Failure> {
        let path = path.into();
        // A path left by a run that was killed would make bind fail with EADDRINUSE. Nothing
        // else can legitimately own this one: it carries our own pid, and a live Muster with
        // this pid is this Muster.
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let listener = UnixListener::bind(&path).map_err(|error| Failure::BindFailed {
            path: path.clone(),
            detail: error.to_string(),
        })?;

        log::info("command.listening", fields! { "path" => &path });

        let closing = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (told, telling) = (Arc::clone(&closing), Arc::clone(&stopped));
        let accept_path = path.clone();
        std::thread::spawn(move || {
            loop {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) => {
                        log::error(
                            "command.accept.failed",
                            fields! {
                                "path" => &accept_path,
                                "detail" => error.to_string(),
                                "impact" => "nothing outside this process can drive this window \
                                             any more, for the rest of the run. The window \
                                             itself is unaffected, so a script will look like \
                                             it is being ignored rather than like Muster is \
                                             broken",
                                "check" => "whether the socket file was deleted underneath the \
                                            app, and the file-descriptor limit for this process",
                            },
                        );
                        telling.store(true, Ordering::Release);
                        return;
                    }
                };
                // The endpoint is going away and this connection is its own doing. After the
                // accept rather than before, because a thread parked in `accept` checks
                // nothing - which is why the drop has to knock first.
                if told.load(Ordering::Acquire) {
                    telling.store(true, Ordering::Release);
                    return;
                }
                // A caller that hangs up mid-answer would otherwise raise SIGPIPE and take the
                // whole window with it. macOS spells it as a socket option.
                set_nosigpipe(&stream);
                std::thread::spawn(move || answer(stream));
            }
        });

        Ok(CommandEndpoint { path, closing, stopped })
    }

    /// The path a caller dials.
    pub fn socket_path(&self) -> &str {
        &self.path
    }

    /// Whether the accepting thread has finished, for a test that needs to prove it does.
    pub fn stopped(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }
}

impl Drop for CommandEndpoint {
    fn drop(&mut self) {
        // Knock, then take the door away, for the reason `PaneControlChannel` does: the
        // accepting thread is parked inside `accept` and nothing else will wake it.
        self.closing.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reads one request, answers it, and hangs up.
fn answer(mut stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));

    let request = match read_frame(&mut stream, LARGEST_MESSAGE) {
        Ok(request) => request,
        Err(detail) => {
            // Debug rather than warn. Anything that can dial a unix socket can produce this,
            // and the caller is the one who needs to hear about it - which it cannot, because
            // by definition it did not manage to ask a question.
            log::debug("command.request.unread", fields! { "detail" => detail });
            return;
        }
    };

    // The same bytes-in, bytes-out call the C ABI makes, including its panic guard: a request
    // arriving here is no more trustworthy than one arriving from the shell.
    let response = dispatch(&request);
    if let Err(error) = write_frame(&mut stream, &response) {
        log::debug(
            "command.answer.unsent",
            fields! {
                "detail" => error.to_string(),
                "impact" => "the caller saw no answer. Whatever it asked for did happen - this \
                             is the reply going missing, not the action",
            },
        );
    }
}

/// Reads a four-byte big-endian length, then that many bytes.
///
/// Public, and used by both ends rather than reimplemented on each: this is a contract with a
/// separate program. The CLI is built from this repo but runs as whatever version somebody has
/// on their PATH, so the framing has to be something both sides agree on without negotiating,
/// and two copies of it would be two chances to disagree. Big-endian because that is what a
/// wire length is everywhere it is not being read by the machine that wrote it.
///
/// `most` is the caller's own idea of what is too big to be worth reading.
pub fn read_frame(stream: &mut impl Read, most: u32) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).map_err(|error| error.to_string())?;
    let length = u32::from_be_bytes(length);
    if length > most {
        return Err(format!(
            "the other end said it was about to send {length} bytes, and {most} is as much as \
             this side will read. Refused without reading, so either this is not a Muster \
             client or the two ends were built against schemas that disagree."
        ));
    }
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload).map_err(|error| error.to_string())?;
    Ok(payload)
}

/// Writes a length and a payload, as [`read_frame`] expects them.
pub fn write_frame(stream: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}

fn set_nosigpipe(stream: &UnixStream) {
    let on: libc::c_int = 1;
    // SAFETY: the fd is owned by `stream` and outlives the call; the option value is an int of
    // the size reported.
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
