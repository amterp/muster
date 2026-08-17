//! Finding a window, and asking it one thing.
//!
//! One request per connection, which is the whole of the protocol: this dials, writes a frame,
//! reads a frame, and hangs up. Nothing here retries. A request that made a pane and then failed
//! to be read back would otherwise be sent twice, and a caller cannot tell the two cases apart
//! from out here.

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{Request, Response};
use prost::Message;

use crate::{Trouble, environment};

/// How long to wait for a window to answer.
///
/// Long, because one of these requests is slow by design: `pane new --run` waits for the new
/// pane's shell to draw a prompt before it types anything, and that wait belongs to the window
/// rather than to whoever asked. Everything else answers in a millisecond, so this is a deadline
/// against a wedged window and not a budget anything spends.
const PATIENCE: Duration = Duration::from_mins(1);

/// Sends one request to a window and hands back what it said.
pub fn ask(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<Response, Trouble> {
    let (path, mut stream) = reach(socket, environment)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));

    write_frame(&mut stream, &request.encode_to_vec()).map_err(|error| {
        Trouble::Unreachable(format!(
            "the window at {path} accepted a connection and then would not take the request \
             ({error}). Either it is shutting down, or something else is listening on that path."
        ))
    })?;
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).map_err(|detail| {
        Trouble::Unreachable(format!(
            "the window at {path} took the request and never answered ({detail}). Whatever was \
             asked for may well have happened - this is the answer going missing, not the action."
        ))
    })?;

    Response::decode(reply.as_slice()).map_err(|error| {
        Trouble::Refused(format!(
            "the window at {path} answered with something this muster cannot read ({error}). The \
             two were built from different schemas, so the app and the `muster` on this PATH come \
             from different versions."
        ))
    })
}

/// Which window to talk to, and a connection to it.
///
/// Connected while deciding rather than after, on purpose: whether a socket answers is the only
/// way to tell a live window from a file a killed one left behind, and connecting twice would
/// leave room for the window to go away in between.
fn reach(
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<(String, UnixStream), Trouble> {
    if let Some(path) = socket {
        return dial(path).map(|stream| (path.to_string(), stream)).map_err(|error| {
            Trouble::Unreachable(format!(
                "nothing is listening on {path} ({error}), which is where --socket said to look."
            ))
        });
    }

    if let Some(path) = environment.get(environment::WINDOW_SOCKET).filter(|p| !p.is_empty()) {
        return dial(path).map(|stream| (path.clone(), stream)).map_err(|error| {
            Trouble::Unreachable(format!(
                "nothing is listening on {path} ({error}), which is the window ${} names. That \
                 window has quit, and this pane outlived it - the pane's daemon kept it running. \
                 Name another with --socket, or open Muster again.",
                environment::WINDOW_SOCKET
            ))
        });
    }

    let mut answered = Vec::new();
    for path in candidates(environment) {
        if let Ok(stream) = dial(&path) {
            answered.push((path, stream));
        }
    }

    match answered.len() {
        1 => Ok(answered.remove(0)),
        0 => Err(Trouble::Unreachable(format!(
            "no Muster window is listening. ${} is not set, so this is not running in a pane \
             Muster made, and nothing under {} answered. Open Muster, or name a window with \
             --socket.",
            environment::WINDOW_SOCKET,
            state_directory(environment).unwrap_or_else(|| "~/.muster/state".to_string())
        ))),
        count => Err(Trouble::Unreachable(format!(
            "{count} Muster windows are listening and nothing says which one this is about: {}. \
             Run this inside one of their panes, where ${} names it, or pick one with --socket.",
            answered.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>().join(", "),
            environment::WINDOW_SOCKET
        ))),
    }
}

fn dial(path: &str) -> std::io::Result<UnixStream> {
    UnixStream::connect(path)
}

/// Every endpoint socket in Muster's state directory, in a settled order.
///
/// Sorted so that a refusal naming several of them reads the same twice in a row - a directory
/// hands them back in whatever order it likes.
fn candidates(environment: &BTreeMap<String, String>) -> Vec<String> {
    let Some(state) = state_directory(environment) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&state) else { return Vec::new() };

    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("command-") && name.ends_with(".sock")
        })
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

/// Where the app puts its endpoint sockets, by the rule `CommandSocketLocation.swift` follows.
fn state_directory(environment: &BTreeMap<String, String>) -> Option<String> {
    environment::muster_home(environment).map(|home| format!("{home}/state"))
}
