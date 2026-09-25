//! Carrying a request to the window that holds what it is about.
//!
//! A tab belongs to exactly one window, and any `muster` verb works from any window (kan
//! a_2Mhi0EZlv): a request naming a tab, or a pane in a tab, that another open window holds is
//! answered by that window. The window a caller happened to reach wraps it in a `Carried`, sends
//! it over the other window's command socket, and relays the answer - so a script never has to
//! know which window holds what, and neither does anything else speaking the schema.
//!
//! Here rather than in the CLI, because the command socket is a door for anything that speaks
//! the schema and the CLI is only one of them. The command socket carries on the thread answering
//! its caller, since a carried request can wait most of a minute on another window. The shell
//! reaches the handler on its main thread, which never waits, and lists only this window's tabs -
//! so the one thing it can ask about another window's tab is a focus from a notification, which
//! [`hand_on`] carries from a thread of its own.
//!
//! Reads are not carried. Every window follows the same daemons, so it can answer a question
//! about any pane itself, and a watch is a stream a relay would have to hold open.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use muster_core::composition::HeldWindow;
use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{Carried, Names, Request, Response, request};
use prost::Message;

use crate::session;

/// How long the other window has to answer.
///
/// The CLI's own patience, because this stands between the CLI and the window it would have
/// reached if it had known where to go: `pane new --run` waits on a shell before it answers.
const PATIENCE: Duration = Duration::from_mins(1);

/// The other open window a request is about, when it is about one.
pub(crate) fn elsewhere(request: &Request) -> Option<HeldWindow> {
    let payload = request.payload.as_ref()?;
    let tab = tab_named(payload)?;
    session::open_window_holding(&tab)
}

/// Hands a focus on another open window's tab to that window, from a thread of its own, and answers
/// at once.
///
/// What a notification click needs: macOS delivers it to whichever instance it chooses, which may
/// not hold the pane, and the shell calls in on its main thread, which never waits on another
/// window. Only a focus, because the shell lists only this window's tabs, so a focus from a
/// notification is the one request it makes about another window's. A request carried here is
/// answered by `route` and never reaches this, so it is not handed back.
pub(crate) fn hand_on(payload: &request::Payload) -> Option<Response> {
    if !matches!(payload, request::Payload::FocusPane(_) | request::Payload::FocusTab(_)) {
        return None;
    }
    let request = Request { payload: Some(payload.clone()) };
    let window = elsewhere(&request)?;
    let to = window.name.to_string();
    let spawned = std::thread::Builder::new().name("carry-focus".to_string()).spawn(move || {
        carry(&window, request);
    });
    if let Err(error) = spawned {
        log::warn(
            "request.carry.unstarted",
            fields! {
                "to" => &to,
                "detail" => error.to_string(),
                "impact" => "the focus was not carried to the window holding the tab, so nothing \
                             moved",
                "check" => "whether this process has run out of threads; go to the pane from \
                            its own window",
            },
        );
        return Some(Response::failure(format!(
            "that is in another window ({to}), and this window could not start carrying the focus \
             there ({error}). Nothing moved."
        )));
    }
    Some(Response::ok())
}

/// Sends a request to the window holding what it is about, and hands back that window's answer.
///
/// A window that cannot be reached is answered here with a refusal that says which window it
/// was, rather than carried out here instead: the tab is that window's, and showing it here is
/// the failure this exists to prevent.
pub(crate) fn carry(window: &HeldWindow, request: Request) -> Vec<u8> {
    let goes_to_a_tab = matches!(
        request.payload,
        Some(request::Payload::FocusPane(_) | request::Payload::FocusTab(_))
    );
    let carried = Request {
        payload: Some(request::Payload::Carried(Box::new(Carried {
            by: session::window_name(),
            request: Some(Box::new(request)),
        }))),
    };
    log::info(
        "request.carried",
        fields! { "to" => window.name.to_string(), "socket" => &window.socket },
    );
    // Before the carry, so the holder has been handed activation by the time it asks for it.
    if goes_to_a_tab {
        session::raise_window(window.pid);
    }
    match exchange(&window.socket, &carried) {
        Ok(answer) => answer,
        Err(detail) => {
            log::warn(
                "request.carry.failed",
                fields! {
                    "to" => window.name.to_string(),
                    "detail" => &detail,
                    "impact" => "the request was about a tab in that window and was not carried \
                                 out, here or there",
                    "check" => "whether that window is still open and answering - `muster window \
                                list` shows the windows that are",
                },
            );
            Response::failure(format!(
                "that is in another window ({}, pid {}), and it did not answer: {detail}. Nothing \
                 was done. If it has quit, the request will go through once it is reopened or the \
                 tab is moved: `muster tab move --tab <tab>`.",
                window.name, window.pid
            ))
            .encode_to_vec()
        }
    }
}

fn exchange(socket: &str, request: &Request) -> Result<Vec<u8>, String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| error.to_string())?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    write_frame(&mut stream, &request.encode_to_vec()).map_err(|error| error.to_string())?;
    read_frame(&mut stream, LARGEST_MESSAGE)
}

/// The tab a request is about, when it is worth carrying.
fn tab_named(payload: &request::Payload) -> Option<TabId> {
    match muster_proto::names(payload)? {
        Names::Tab(tab) => Some(TabId::new(tab)),
        Names::Pane(pane) => session::tab_of_pane(&PaneId::new(pane)),
    }
}
