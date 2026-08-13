//! The pane this window's keyboard feeds, and how it got there.
//!
//! Small on purpose. `PaneInput` is the state machine - keymap, then encoder, then out -
//! and it lives in the core where the corpus judges it. What is here is the part that only
//! makes sense with a shell attached: which pane, on which socket, with which daemon behind
//! it, and the fact that there may be no pane at all.

use std::sync::{Arc, Mutex};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{Keymap, PaneInput, TerminalModeProfile};
use muster_herdr::{HerdrPaneChannel, PaneControlChannel};
use muster_vt::KeyEncoder;

use crate::ffi;
use crate::proto::{Event, PaneTypeable, event};

/// Everything one attached pane needs.
#[derive(Debug)]
pub(crate) struct Pane {
    pub(crate) input: PaneInput,
    pub(crate) control_socket_path: String,
    pub(crate) server_encoded: bool,
    /// Held because dropping it unlinks the socket and stops the listener.
    _control: Arc<PaneControlChannel>,
}

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
    })
}

/// The moment the pane becomes typeable, on the thread that accepted the connection.
fn typeable(pane_id: &str) {
    ffi::emit(&Event {
        payload: Some(event::Payload::PaneTypeable(PaneTypeable { pane_id: pane_id.to_string() })),
    });
}
