//! Finding a window, and asking it one thing.
//!
//! One request per connection, which is the whole of the protocol: this dials, writes a frame,
//! reads a frame, and hangs up - or, for a watch, reads frames until the window stops sending
//! them. Nothing here retries. A request that made a pane and then failed to be read back would
//! otherwise be sent twice, and a caller cannot tell the two cases apart from out here.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{Request, Response};
use prost::Message;

use crate::{Trouble, environment};

/// How long to wait for a window to answer.
///
/// A deadline against a wedged window rather than a budget anything spends: a window answers in
/// milliseconds.
const PATIENCE: Duration = Duration::from_mins(1);

/// Sends one request to a window and hands back what it said.
pub fn ask(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<Response, Trouble> {
    ask_within(request, socket, environment, PATIENCE)
}

/// The same, under a deadline the caller sets.
///
/// Here because [`PATIENCE`] is a minute, so what a lost answer exits with cannot be proved by a
/// test that has to wait one out.
pub fn ask_within(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
    patience: Duration,
) -> Result<Response, Trouble> {
    let any_will_do = request.payload.as_ref().is_some_and(muster_proto::any_window_will_do);
    let (path, stream) = reach(socket, environment, any_will_do)?;
    exchange(&path, stream, &from_here(request, environment), patience)
}

/// A watch that has been sent, and the answers still to come on its connection.
#[derive(Debug)]
pub struct Answers {
    path: String,
    stream: UnixStream,
}

/// Why a watch's answers stopped without one that ends it.
#[derive(Debug)]
pub enum Ended {
    /// The window closed the connection without saying the watch was over, which is a window
    /// quitting.
    HungUp(String),
    /// The caller's own deadline came first.
    TimedOut,
}

/// Sends a request that is answered with a stream, and hands back the connection to read it from.
///
/// Found the way [`ask`] finds a window, and refused the same ways before anything is written.
pub fn follow(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<Answers, Trouble> {
    let (path, mut stream) = reach(socket, environment, false)?;
    let _ = stream.set_write_timeout(Some(PATIENCE));
    write_frame(&mut stream, &from_here(request, environment).encode_to_vec()).map_err(
        |error| {
            Trouble::Unreachable(format!(
                "the window at {path} accepted a connection and then would not take the watch \
             ({error}). Either it is shutting down, or something else is listening on that path."
            ))
        },
    )?;
    Ok(Answers { path, stream })
}

impl Answers {
    /// The next answer, waiting until `deadline` for it, or forever with none.
    pub fn next(&mut self, deadline: Option<Instant>) -> Result<Response, Ended> {
        // A zero timeout is refused by the socket, so a deadline already past waits a
        // millisecond - which is also what lets an answer already in the buffer through.
        let wait = deadline.map(|deadline| {
            deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))
        });
        let _ = self.stream.set_read_timeout(wait);
        let path = &self.path;
        let bytes = read_frame(&mut self.stream, LARGEST_MESSAGE).map_err(|detail| {
            // The error's own kind is lost in the framing's string, and a deadline that has
            // passed is the one read failure a caller asked for.
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                Ended::TimedOut
            } else {
                Ended::HungUp(format!(
                    "the window at {path} hung up in the middle of the watch ({detail}), which \
                     is what a window that quits does. Watching changed nothing, so run it again \
                     once a window is back."
                ))
            }
        })?;
        Response::decode(bytes.as_slice()).map_err(|error| {
            Ended::HungUp(format!(
                "the window at {path} sent something this muster cannot read ({error}), so the \
                 two were built from different schemas. Reach the running app's own copy at \
                 ~/.muster/bin/muster."
            ))
        })
    }
}

/// Every window listening on this machine, and what each one answers.
///
/// One connection each, and a window that will not answer is kept in the list with its reason
/// rather than dropped: a caller looking for a window it cannot find is better served by "this
/// one is there and did not answer" than by a shorter list.
///
/// Deliberately not routed through [`reach`]. That refuses when several windows answer, which
/// is the right answer to "drive a window" and the wrong one to "which windows are there".
pub fn survey(
    environment: &BTreeMap<String, String>,
    request: &Request,
) -> Vec<(String, Result<Response, Trouble>)> {
    survey_of(candidates(environment), &from_here(request, environment))
}

/// [`survey`], of the apps [`around`] finds: what a pane asks when its own app has gone.
pub fn survey_around(
    environment: &BTreeMap<String, String>,
    request: &Request,
) -> Vec<(String, Result<Response, Trouble>)> {
    survey_of(around(environment), &from_here(request, environment))
}

/// [`survey`], of these windows.
fn survey_of(windows: Vec<String>, request: &Request) -> Vec<(String, Result<Response, Trouble>)> {
    windows
        .into_iter()
        .filter_map(|path| match dial(&path) {
            Ok(stream) => Some((path.clone(), exchange(&path, stream, request, PATIENCE))),
            // Not an entry. A socket nothing is listening on is a window that has gone, and
            // the file outliving it is ordinary - a killed Muster never unlinks its own.
            Err(_) => None,
        })
        .collect()
}

/// The request, saying which pane it was sent from when this runs in one.
///
/// So that the app answers from the window holding that pane's tab, which is not always the window
/// the pane's socket reaches: a tab moved to another window keeps the socket it was started with,
/// and one app can hold several windows (MIP-6). Left as it is when the request already says, or
/// when this runs outside a pane.
fn from_here<'a>(request: &'a Request, environment: &BTreeMap<String, String>) -> Cow<'a, Request> {
    match environment.get(environment::PANE_NAME).filter(|pane| !pane.is_empty()) {
        Some(pane) if request.from_pane.is_empty() => {
            let mut sent = request.clone();
            sent.from_pane.clone_from(pane);
            Cow::Owned(sent)
        }
        _ => Cow::Borrowed(request),
    }
}

/// One request down a connection already made, and the answer back.
fn exchange(
    path: &str,
    mut stream: UnixStream,
    request: &Request,
    patience: Duration,
) -> Result<Response, Trouble> {
    let _ = stream.set_read_timeout(Some(patience));
    let _ = stream.set_write_timeout(Some(patience));

    let encoded = request.encode_to_vec();
    // Refused here rather than written. A window reads the length, refuses it and hangs up
    // without answering, and a request with no answer is exit 4 - "it may well have happened" -
    // about something that certainly did not.
    if encoded.len() > LARGEST_MESSAGE as usize {
        return Err(Trouble::Refused(format!(
            "this request is {} bytes and a window reads at most {LARGEST_MESSAGE}, so nothing \
             was sent. A `pane send --file` of a large file is the usual way to get here; send \
             a pointer to the file instead, such as `read <path> and follow it`.",
            encoded.len()
        )));
    }

    // Unreachable rather than unanswered, and the difference is the frame: a write that did not
    // finish leaves a length the window will wait out and discard, so the request was not carried
    // out and sending it again costs nothing.
    write_frame(&mut stream, &encoded).map_err(|error| {
        Trouble::Unreachable(format!(
            "the window at {path} accepted a connection and then would not take the request \
             ({error}). Either it is shutting down, or something else is listening on that path."
        ))
    })?;
    // Everything below here is unanswered rather than refused or unreachable. The request is on
    // the window's side of the socket, so whatever it asks for may already have happened, and a
    // caller that reads this as a failure and sends it again is asking for it twice.
    let reply = read_frame(&mut stream, LARGEST_MESSAGE).map_err(|detail| {
        Trouble::Unanswered(format!(
            "the window at {path} took the request and never answered ({detail}). Whatever was \
             asked for may well have happened - this is the answer going missing, not the \
             action - so send it again only if doing it twice is harmless. `muster pane read` \
             says what a pane has on it."
        ))
    })?;

    Response::decode(reply.as_slice()).map_err(|error| {
        Trouble::Unanswered(format!(
            "the window at {path} answered with something this muster cannot read ({error}). The \
             two were built from different schemas, so the app and the `muster` on this PATH come \
             from different versions. It answered, so whatever was asked for may well have \
             happened; reach the running app's own copy at ~/.muster/bin/muster rather than \
             sending this again."
        ))
    })
}

/// Which window to talk to, and a connection to it.
///
/// Connected while deciding rather than after, on purpose: whether a socket answers is the only
/// way to tell a live window from a file a killed one left behind, and connecting twice would
/// leave room for the window to go away in between.
///
/// `any_will_do` is whether any open window can carry the request out
/// (`muster_proto::any_window_will_do`): one naming its tab or pane, which a window carries to the
/// window holding it, or a tab move naming where it goes. With several open, the first that
/// answers is asked, and it does not matter which.
fn reach(
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
    any_will_do: bool,
) -> Result<(String, UnixStream), Trouble> {
    if let Some(path) = socket {
        return dial(path).map(|stream| (path.to_string(), stream)).map_err(|error| {
            Trouble::Unreachable(format!(
                "nothing is listening on {path} ({error}), which is where --socket said to look."
            ))
        });
    }

    // A pane whose own window has quit asks the windows open beside it now: Muster relaunched,
    // and one of them holds the pane's tab (`siblings`).
    let (around, nobody) = match environment.get(environment::WINDOW_SOCKET) {
        Some(path) if !path.is_empty() => match dial(path) {
            Ok(stream) => return Ok((path.clone(), stream)),
            Err(error) => (
                siblings(path),
                format!(
                    "nothing is listening on {path} ({error}), which is the window ${} names, and \
                     no other window of Muster is listening beside it. That window has quit, and \
                     this pane outlived it - the pane's daemon kept it running. Open Muster \
                     again, or name a window with --socket.",
                    environment::WINDOW_SOCKET
                ),
            ),
        },
        _ => (
            candidates(environment),
            format!(
                "no Muster window is listening. ${} is not set, so this is not running in a pane \
                 Muster made, and nothing under {} answered. Open Muster, or name a window with \
                 --socket.",
                environment::WINDOW_SOCKET,
                state_directory(environment).unwrap_or_else(|| "~/.muster/state".to_string())
            ),
        ),
    };

    let mut answered = Vec::new();
    for path in around {
        if let Ok(stream) = dial(&path) {
            answered.push((path, stream));
        }
    }

    match answered.len() {
        1 => Ok(answered.remove(0)),
        count if count > 1 && any_will_do => Ok(answered.remove(0)),
        0 => Err(Trouble::Unreachable(nobody)),
        count => Err(Trouble::Unreachable(format!(
            "{count} Muster apps are listening and nothing says which one this is about: {}. \
             Run this inside one of their panes, where ${} names it, or pick one with --socket.",
            answered.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>().join(", "),
            environment::WINDOW_SOCKET
        ))),
    }
}

/// Whether the window a caller who named none means would answer: the one `$MUSTER_SOCKET`
/// names or, when that has quit, one beside it; or with that unset, any in Muster's state
/// directory.
///
/// Asked only after a request found no window, to tell "nothing is listening" - when this
/// machine's daemon can answer instead - from "several are, and nothing says which", which is
/// the caller's to settle.
pub fn any_window_answers(environment: &BTreeMap<String, String>) -> bool {
    match environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty()) {
        Some(path) => dial(path).is_ok() || siblings(path).iter().any(|path| dial(path).is_ok()),
        None => candidates(environment).iter().any(|path| dial(path).is_ok()),
    }
}

/// Whether the window `$MUSTER_SOCKET` names is gone: set, and nothing answering there.
pub fn own_window_gone(environment: &BTreeMap<String, String>) -> bool {
    environment
        .get(environment::WINDOW_SOCKET)
        .is_some_and(|path| !path.is_empty() && dial(path).is_err())
}

/// The windows to ask when nobody named one: beside a pane's own window when that has quit,
/// and otherwise every window in Muster's state directory.
pub fn around(environment: &BTreeMap<String, String>) -> Vec<String> {
    match environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty()) {
        Some(path) => siblings(path),
        None => candidates(environment),
    }
}

/// The other windows of the same Muster listening beside `path`, the socket of a window that
/// has quit: every socket in its directory whose name shares everything up to its last `-`.
///
/// That is `command-*.sock` in the state directory of this install on this machine, and
/// `window-<install>-*.sock` beside the install's daemon on a machine attached over ssh, where
/// each window forwards its own socket. A window's socket is named after its process, so a
/// relaunched Muster listens beside the one a pane was told of rather than at it - and the pane
/// is reached from there, since every window carries a request naming a pane to the window
/// holding that pane's tab. A name with no `-` has no siblings.
///
/// A pane made before a devenv's windows carried their install in the name holds
/// `window-<window>.sock`, whose siblings are every install's windows there.
pub fn siblings(path: &str) -> Vec<String> {
    let path = std::path::Path::new(path);
    let (Some(directory), Some(own)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return Vec::new();
    };
    let Some((kind, _)) = own.rsplit_once('-') else { return Vec::new() };
    let kind = format!("{kind}-");
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name != own && name.starts_with(&kind) && name.ends_with(".sock")
        })
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn dial(path: &str) -> std::io::Result<UnixStream> {
    UnixStream::connect(path)
}

/// Every endpoint socket in Muster's state directory, in a settled order: one per app, which is
/// one per install running under this home.
///
/// Public so that making a window can wait for one to appear that was not here before.
///
/// Sorted so that a refusal naming several of them reads the same twice in a row - a directory
/// hands them back in whatever order it likes.
pub fn candidates(environment: &BTreeMap<String, String>) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use muster_proto::{ReadWindow, request};

    use super::*;

    fn read_window() -> Request {
        Request::new(request::Payload::ReadWindow(ReadWindow::default()))
    }

    fn running_in(pane: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(environment::PANE_NAME.to_string(), pane.to_string())])
    }

    #[test]
    fn a_request_from_a_pane_says_which() {
        assert_eq!(from_here(&read_window(), &running_in("p1w3r07bsd")).from_pane, "p1w3r07bsd");
    }

    #[test]
    fn a_request_from_outside_a_pane_names_none() {
        assert_eq!(from_here(&read_window(), &BTreeMap::new()).from_pane, "");
        assert_eq!(from_here(&read_window(), &running_in("")).from_pane, "");
    }

    #[test]
    fn a_pane_the_request_already_names_is_kept() {
        let mut asked = read_window();
        asked.from_pane = "p2".to_string();
        assert_eq!(from_here(&asked, &running_in("p1")).from_pane, "p2");
    }
}
