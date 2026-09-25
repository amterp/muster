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
//!
//! A `WatchPanes` is the one request answered more than once, because its caller is the one
//! that does not exit: it is waiting for agents to change state, and polling for that is what it
//! exists to replace (kan a_2M9T8O6dL). Still one request per connection; the answers keep
//! coming on it until the watch ends or the caller hangs up.
//!
//! One answer is held back rather than decided differently: a pane a request made is answered
//! once this window holds that pane ([`after_the_window_holds_it`]). That changes when the answer
//! is written and not what the request did, and the reason is this transport's own - a caller on
//! it hears no events, only the answer.

use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::mirror::backend::PaneId;
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{Request, Response, WatchPanes, request, response};
use prost::Message;

use crate::watch::Next;
use crate::{dispatch, forward, handler, session};

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

    // Decoded here only to tell a watch from everything else. Which transport shape a request
    // needs is this file's business; what the request means is still the handler's.
    if let Ok(Request { payload: Some(request::Payload::WatchPanes(watching)) }) =
        Request::decode(request.as_slice())
    {
        follow(stream, &watching);
        return;
    }

    // A request about another window's tab is that window's to answer (`forward`).
    let carried = Request::decode(request.as_slice())
        .ok()
        .and_then(|decoded| Some((forward::elsewhere(&decoded)?, decoded)));

    // The same bytes-in, bytes-out call the C ABI makes, including its panic guard: a request
    // arriving here is no more trustworthy than one arriving from the shell.
    let response = match carried {
        Some((window, decoded)) => forward::carry(&window, decoded),
        None => dispatch(&request),
    };
    after_the_window_holds_it(&response);
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

/// Holds back an answer naming a pane the request made until this window holds that pane.
///
/// A split answers as soon as the daemon has made the pane, and the daemon's event describing it
/// reaches the window up to a herdr event pass later. A caller here names the pane in its next
/// command - `muster pane read --pane "$(muster pane new)"` - and every lookup in the window
/// refused a name it had not heard of yet (kan a_2P5nkSS8g). Waiting once here answers that for
/// every verb, including ones written after this.
///
/// Not in the handler, because the shell reaches that on its main thread. The shell learns of the
/// pane from the event and never names one before then, so a wait there would stop the window
/// drawing and taking input for nothing.
///
/// A pane still unheard of at the deadline is answered anyway. The daemon made it, and a caller
/// told otherwise would make another.
fn after_the_window_holds_it(response: &[u8]) {
    let Ok(Response { payload: Some(response::Payload::Made(made)) }) = Response::decode(response)
    else {
        return;
    };
    let pane = PaneId::new(&made.pane_id);
    let asked = Instant::now();
    while session::daemon_holding(&pane).is_none() {
        if asked.elapsed() >= TURNS_UP_WITHIN {
            log::warn(
                "pane.made.unheard",
                fields! {
                    "pane" => pane.to_string(),
                    "waited_ms" => asked.elapsed().as_millis().to_string(),
                    "impact" => "the pane exists and its name was answered, and a command naming \
                                 it straight away may be refused until this window hears of it",
                    "check" => "whether this window is still hearing from the pane's daemon - \
                                `muster window` shows each daemon's state - and whether the pane \
                                closed as soon as it was made",
                },
            );
            return;
        }
        std::thread::sleep(HEARD_OF_POLL);
    }
}

/// How long a pane a request made is given to reach this window before it is answered anyway.
///
/// A pane arrives within one herdr event pass, a tenth of a second, on a machine keeping up. Not
/// measured beyond that: the whole wait is paid only when the window is not hearing from the
/// daemon, and then nothing that names the pane would work however long this was.
const TURNS_UP_WITHIN: Duration = Duration::from_secs(2);

/// How often the window is asked, short because every `pane new` pays up to one interval of it.
const HEARD_OF_POLL: Duration = Duration::from_millis(5);

/// How long a watch with nothing to say goes before checking its caller is still there.
///
/// A watch finds out its caller has gone when it next writes, and one waiting on a pane that
/// stays busy for an hour would otherwise hold a thread for that hour after `muster pane wait`
/// was interrupted. A second is one syscall per open watch per second.
const HANGUP_CHECK: Duration = Duration::from_secs(1);

/// Answers a watch until it ends, the caller hangs up, or the window goes away.
fn follow(mut stream: UnixStream, watching: &WatchPanes) {
    let mut watch = match handler::watch_panes(watching) {
        Ok(watch) => watch,
        Err(refusal) => {
            let _ = write_frame(&mut stream, &refusal.encode_to_vec());
            return;
        }
    };
    loop {
        let (response, last) = match watch.next(HANGUP_CHECK) {
            Next::Answer(response) => (response, false),
            Next::Last(response) => (response, true),
            Next::Quiet if hung_up(&stream) => return,
            Next::Quiet => continue,
            Next::Over => return,
        };
        if write_frame(&mut stream, &response.encode_to_vec()).is_err() || last {
            return;
        }
    }
}

/// Whether a watch's caller has hung up, asked without blocking and without consuming anything.
///
/// A caller sends its one request and then only reads, so anything readable on its end is either
/// the end of the stream or a caller not speaking this protocol - and both mean stop.
fn hung_up(stream: &UnixStream) -> bool {
    let mut byte = 0u8;
    // SAFETY: the fd is owned by `stream` and outlives the call; the buffer is one byte we own,
    // and MSG_PEEK leaves whatever is there in place.
    let read = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            std::ptr::from_mut(&mut byte).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if read >= 0 {
        return true;
    }
    !matches!(
        std::io::Error::last_os_error().kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
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
