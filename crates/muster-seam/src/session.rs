//! The pane this window's keyboard feeds, and how it got there.
//!
//! Small on purpose. `PaneInput` is the state machine - keymap, then encoder, then out -
//! and it lives in the core where the corpus judges it. What is here is the part that only
//! makes sense with a shell attached: which pane, on which socket, with which daemon behind
//! it, and the fact that there may be no pane at all.

use std::sync::{Arc, LazyLock, Mutex};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{Keymap, PaneInput, TerminalModeProfile};
use muster_core::mirror::{Change, Mirror};
use muster_herdr::subscription::{Notice, Subscription};
use muster_herdr::{HerdrPaneChannel, PaneControlChannel, discover_socket_path};
use muster_vt::KeyEncoder;

use crate::ffi;
use crate::proto::{BackendHealth, Event, PaneStateChanged, PaneTypeable, event};

/// Everything one attached pane needs.
#[derive(Debug)]
pub(crate) struct Pane {
    pub(crate) input: PaneInput,
    pub(crate) control_socket_path: String,
    pub(crate) server_encoded: bool,
    /// Held because dropping it unlinks the socket and stops the listener.
    _control: Arc<PaneControlChannel>,
    /// Held because dropping it ends the subscription and its threads.
    ///
    /// Per attached pane, which is backwards: the mirror describes a daemon and there is
    /// one of those however many panes a window shows. It is the cheap version until
    /// composition inverts it, and it costs a duplicate subscription only in the case a
    /// window attaches twice - which today it cannot.
    _mirror: Option<Subscription>,
}

/// The core's picture of the daemon behind the attached pane.
///
/// Static because there is one window and one daemon today. Both of those change in the
/// composition chunk, and this becomes a map keyed by daemon; nothing reads it in a way
/// that assumes otherwise.
pub(crate) static MIRROR: LazyLock<Arc<Mutex<Mirror>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Mirror::new())));

/// The window's pane, or none - which is what a bare `muster` has.
pub(crate) static PANE: Mutex<Option<Pane>> = Mutex::new(None);

/// Why a pane could not be attached.
pub(crate) enum AttachError {
    NoSocket(String),
    NoEncoder(String),
}

/// Opens the listener, finds the daemon if there is one, and builds the input path.
///
/// The socket is bound before this returns, and so before the shell creates the surface
/// that spawns the bridge - which is what stops the bridge losing a race against its own
/// listener.
pub(crate) fn attach(pane_id: &str) -> Result<Pane, AttachError> {
    // The core's listener, so the core names it. A pid in the name because nothing else can
    // legitimately own this path, which is what makes unlinking a stale one safe.
    let path = std::env::temp_dir().join(format!("muster-{}.sock", std::process::id()));
    let path = path.to_string_lossy().into_owned();

    let announced = pane_id.to_string();
    let control = PaneControlChannel::bind(path.clone(), move || typeable(&announced))
        .map_err(|error| AttachError::NoSocket(error.to_string()))?;
    let control = Arc::new(control);

    // The second channel, for the keys and text whose correct encoding depends on modes the
    // control stream cannot show us. Optional on purpose: no daemon socket means the pane
    // still works, with a guess.
    let server = HerdrPaneChannel::discover(pane_id);
    if server.is_none() {
        log::warn(
            "app.server_channel.unavailable",
            fields! {
                "impact" => "arrow keys and paste fall back to a guessed encoding, which \
                             pagers reject and multi-line pastes run as commands",
            },
        );
    }
    let server_encoded = server.is_some();

    // The pane's modes are not readable, so this is the documented guess. One day it is fed
    // from the daemon; nothing above here changes when it is.
    let encoder = KeyEncoder::new(TerminalModeProfile::UNKNOWN_PANE)
        .map_err(|error| AttachError::NoEncoder(error.to_string()))?;

    Ok(Pane {
        input: PaneInput::new(
            Arc::clone(&control) as Arc<_>,
            server.map(|channel| Arc::new(channel) as Arc<_>),
            Arc::new(encoder),
            Keymap::default(),
        ),
        control_socket_path: path,
        server_encoded,
        _control: control,
        _mirror: mirror(),
    })
}

/// Starts following the daemon, if there is one to follow.
///
/// `None` is not a failure: the renderer check runs with no daemon at all, and a pane still
/// types and renders without one. What is lost is the agent state, which the window then
/// reports as `disconnected` rather than as nothing.
fn mirror() -> Option<Subscription> {
    let socket_path = discover_socket_path(&std::env::vars().collect())?;
    Some(Subscription::start(socket_path, Arc::clone(&MIRROR), Arc::new(announce)))
}

/// Turns what the daemon said into a log line and, where the window renders it, an event.
///
/// The whole of D's answer to "agent states are the point": every pane's transitions reach
/// the log, and the attached one's reaches the chrome. Which pane the window is showing is
/// the shell's business, so every pane is sent and the shell decides.
fn announce(notice: Notice) {
    match notice {
        Notice::Bootstrapped { changes, dropped } => {
            log::info(
                "mirror.bootstrap",
                fields! {
                    "changes" => changes.len().to_string(),
                    "dropped" => dropped.to_string(),
                },
            );
            if dropped > 0 {
                log::warn(
                    "mirror.entries_dropped",
                    fields! {
                        "count" => dropped.to_string(),
                        "impact" => "the session renders with fewer panes than the daemon \
                                     holds, which looks like panes the user closed",
                        "check" => "a herdr whose pane, tab or workspace payload has moved - \
                                    compare corpus/herdr-<version>/api-schema.json",
                    },
                );
            }
            health("connected", "");
            for change in changes {
                report(&change);
            }
        }
        Notice::Changed(change) => report(&change),
        Notice::Stale { detail } => {
            log::warn(
                "backend.stale",
                fields! {
                    "detail" => detail.clone(),
                    "impact" => "panes keep rendering and the agent states shown are now a \
                                 guess about the present",
                    "check" => "whether the daemon is still running, and for a remote one \
                                whether the tunnel is up",
                },
            );
            health("stale", &detail);
        }
        Notice::Reconnected => {
            log::info("backend.reconnected", fields! {});
            health("connected", "");
        }
        Notice::UnknownEvent { kind } => log::warn(
            "backend.unknown_event",
            fields! {
                "kind" => kind,
                "impact" => "whatever this event reports is not reaching the mirror, so the \
                             view is missing that kind of change entirely",
                "check" => "whether this herdr is newer than the pinned one - if the event \
                            matters, it needs reading in muster-herdr's decoder",
            },
        ),
    }
}

fn report(change: &Change) {
    if let Change::AgentStateChanged { pane, from, to } = change {
        log::info(
            "agent.state",
            fields! {
                "pane" => pane.to_string(),
                "from" => from.as_str(),
                "to" => to.as_str(),
            },
        );
        ffi::emit(&Event {
            payload: Some(event::Payload::PaneStateChanged(PaneStateChanged {
                pane_id: pane.to_string(),
                state: to.as_str().to_string(),
            })),
        });
    }
}

fn health(state: &str, detail: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::BackendHealth(BackendHealth {
            state: state.to_string(),
            detail: detail.to_string(),
        })),
    });
}

/// The moment the pane becomes typeable, on the thread that accepted the connection.
fn typeable(pane_id: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneTypeable(PaneTypeable { pane_id: pane_id.to_string() })),
    });
}
