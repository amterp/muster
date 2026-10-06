//! Finding a window, and asking it one thing.
//!
//! One request per connection, which is the whole of the protocol: this dials, writes a frame,
//! reads a frame, and hangs up - or, for a watch, reads frames until the window stops sending
//! them. Nothing here retries. A request that made a pane and then failed to be read back would
//! otherwise be sent twice, and a caller cannot tell the two cases apart from out here.
//!
//! Every window is asked what it is showing before it is asked anything else, on a connection of
//! its own and with [`ALIVE_WITHIN`] to answer. A socket that accepts is not yet a window: sshd
//! forwarding one from a laptop that has gone to sleep accepts and never answers. The question
//! changes nothing, so a window that does not answer it in time is passed over with nothing sent,
//! and its answer says which tabs it holds when several windows answer.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use muster_proto::frame::{LARGEST_MESSAGE, read_frame, write_frame};
use muster_proto::{Names, ReadWindow, Request, Response, request, response};
use prost::Message;

use crate::{Trouble, environment};

/// How long to wait for a window to answer.
///
/// A deadline against a wedged window rather than a budget anything spends: a window answers in
/// milliseconds.
const PATIENCE: Duration = Duration::from_mins(1);

/// How long a window has to answer what it is showing before it is passed over as not there.
///
/// A window answers that from memory: milliseconds here, and a round trip more over ssh. What
/// takes longer is a socket with nobody behind it that still accepts, which is what a devenv
/// holds while the laptop forwarding a window to it sleeps (kan a_2ZNnSyiXR). Every command run
/// there pays this once while the laptop sleeps, rather than a minute.
const ALIVE_WITHIN: Duration = Duration::from_secs(2);

/// Sends one request to a window and hands back what it said.
pub fn ask(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
) -> Result<Response, Trouble> {
    ask_within(request, socket, environment, PATIENCE)
}

/// The same, under a deadline the caller sets, which bounds the question asked first too.
///
/// Here because [`PATIENCE`] is a minute, so what a lost answer exits with cannot be proved by a
/// test that has to wait one out.
pub fn ask_within(
    request: &Request,
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
    patience: Duration,
) -> Result<Response, Trouble> {
    let request = from_here(request, environment);
    fits(&request)?;
    let probe = probe(environment);
    let live = reach(socket, environment, &request, &probe, patience)?;
    if *request == probe {
        return live.answer.and_then(|response| as_asked(&live.path, &request, response));
    }
    let stream = live.connect()?;
    exchange(&live.path, stream, &request, patience)
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
    let request = from_here(request, environment);
    let live = reach(socket, environment, &request, &probe(environment), PATIENCE)?;
    let path = live.path.clone();
    let mut stream = live.connect()?;
    let _ = stream.set_write_timeout(Some(PATIENCE));
    write_frame(&mut stream, &request.encode_to_vec()).map_err(|error| {
        Trouble::Unreachable(format!(
            "the window at {path} accepted a connection and then would not take the watch \
             ({error}). Either it is shutting down, or something else is listening on that path."
        ))
    })?;
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
/// One connection each, and a window that is there and will not answer is kept in the list with
/// its reason rather than dropped: a caller looking for a window it cannot find is better served
/// by "this one is there and did not answer" than by a shorter list.
///
/// Deliberately not routed through [`reach`]. That refuses when several windows answer, which
/// is the right answer to "drive a window" and the wrong one to "which windows are there".
pub fn survey(
    environment: &BTreeMap<String, String>,
    request: &Request,
) -> Vec<(String, Result<Response, Trouble>)> {
    let surveyed = survey_of(candidates(environment), request, environment);
    let mut answers = surveyed.answers;
    answers.extend(surveyed.missed.into_iter().filter_map(|(path, miss)| match miss {
        // Not an entry. A socket nothing is listening on is a window that has gone, and the
        // file outliving it is ordinary - a killed Muster never unlinks its own.
        Miss::Gone(_) | Miss::HungUp(_) => None,
        Miss::Silent(_) | Miss::Forbidden(_) => {
            let said = miss.said(&path);
            Some((path, Err(Trouble::Unreachable(said))))
        }
    }));
    answers.sort_by(|(one, _), (other, _)| one.cmp(other));
    answers
}

/// What asking several windows came to.
#[derive(Debug)]
pub struct Surveyed {
    /// Each window that answered what it is showing, and what it answered the request.
    pub answers: Vec<(String, Result<Response, Trouble>)>,
    /// The windows that did not, and why.
    missed: Vec<(String, Miss)>,
}

impl Surveyed {
    /// Why nobody answered, when nobody did and some window there took the connection and said
    /// nothing: asking again would only wait on it again, so the caller goes to the daemon now.
    /// None when anything answered, or nothing was there, which asking a window says as usual.
    pub fn nobody_answered(&self, environment: &BTreeMap<String, String>) -> Option<Trouble> {
        if !self.answers.is_empty() || silent(&self.missed).is_empty() {
            return None;
        }
        let own = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
        Some(nobody(environment, own.map(String::as_str), &self.missed))
    }
}

/// [`survey`], of the windows [`around`] finds: what a pane asks when its own app has gone, and a
/// caller outside every pane asks of every app. A window that does not answer in time is left out
/// of the answers.
pub fn survey_around(environment: &BTreeMap<String, String>, request: &Request) -> Surveyed {
    let mut surveyed = survey_of(around(environment), request, environment);
    // Gone, or this would not have been asked around. Said first, as `reach` says it.
    if let Some(own) = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty())
        && let Err(error) = dial(own)
    {
        surveyed.missed.insert(0, (own.clone(), Miss::of(error)));
    }
    surveyed
}

/// [`survey`], of these windows: asked what they are showing all at once, and then the request,
/// one at a time, of each that answered.
fn survey_of(
    windows: Vec<String>,
    request: &Request,
    environment: &BTreeMap<String, String>,
) -> Surveyed {
    let request = from_here(request, environment);
    let probe = probe(environment);
    let (live, missed) = probe_all(windows, &probe, ALIVE_WITHIN);
    let answers = live
        .into_iter()
        .map(|live| {
            let answer = if *request == probe {
                live.answer.and_then(|response| as_asked(&live.path, &request, response))
            } else {
                live.connect().and_then(|stream| exchange(&live.path, stream, &request, PATIENCE))
            };
            (live.path, answer)
        })
        .collect();
    Surveyed { answers, missed }
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

/// What every window is asked first: what it is showing, from here.
fn probe(environment: &BTreeMap<String, String>) -> Request {
    from_here(&Request::new(request::Payload::ReadWindow(ReadWindow::default())), environment)
        .into_owned()
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

    let encoded = fits(request)?;

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
    let response = decoded(path, &reply)?;
    as_asked(path, request, response)
}

/// The request as written, or a refusal for one too big for any window to read.
///
/// Refused rather than written. A window reads the length, refuses it and hangs up without
/// answering, and a request with no answer is exit 4 - "it may well have happened" - about
/// something that certainly did not. Checked before any window is dialled, too, since no window
/// would read it.
fn fits(request: &Request) -> Result<Vec<u8>, Trouble> {
    let encoded = request.encode_to_vec();
    if encoded.len() > LARGEST_MESSAGE as usize {
        return Err(Trouble::Refused(format!(
            "this request is {} bytes and a window reads at most {LARGEST_MESSAGE}, so nothing \
             was sent. A `pane send --file` of a large file is the usual way to get here; send \
             a pointer to the file instead, such as `read <path> and follow it`.",
            encoded.len()
        )));
    }
    Ok(encoded)
}

/// A window's answer, read.
fn decoded(path: &str, reply: &[u8]) -> Result<Response, Trouble> {
    Response::decode(reply).map_err(|error| {
        Trouble::Unanswered(format!(
            "the window at {path} answered with something this muster cannot read ({error}). The \
             two were built from different schemas, so the app and the `muster` on this PATH come \
             from different versions. It answered, so whatever was asked for may well have \
             happened; reach the running app's own copy at ~/.muster/bin/muster rather than \
             sending this again."
        ))
    })
}

/// Refuses an answer from a window that ignored a field it predates and answered another
/// question: a read of a pane's last turn answered with its newest rows reads as the report.
fn as_asked(path: &str, request: &Request, response: Response) -> Result<Response, Trouble> {
    let asked_turn = matches!(&request.payload,
        Some(request::Payload::ReadPane(read)) if read.turn);
    let read_turn = matches!(&response.payload,
        Some(response::Payload::PaneText(read)) if read.turn);
    if asked_turn && !read_turn && matches!(&response.payload, Some(response::Payload::PaneText(_)))
    {
        return Err(Trouble::Refused(format!(
            "the window at {path} predates reading what a pane's agent printed in its last turn, \
             and answered with the pane's newest rows instead. Read with --rows, or with \
             --no-window to ask the daemon directly."
        )));
    }
    Ok(response)
}

// ---------------------------------------------------------------------------------------------
// Which window

/// A window that answered what it is showing.
#[derive(Debug)]
struct Live {
    path: String,
    /// Its answer, or why it could not be read: an answer all the same, so the window is there.
    answer: Result<Response, Trouble>,
}

impl Live {
    /// What it said it is showing, when it said so in a way this muster reads.
    fn window(&self) -> Option<&muster_proto::Window> {
        match &self.answer {
            Ok(Response { payload: Some(response::Payload::Window(window)) }) => Some(window),
            _ => None,
        }
    }

    /// A connection of its own for the request, since one carries one request.
    fn connect(&self) -> Result<UnixStream, Trouble> {
        dial(&self.path).map_err(|error| {
            Trouble::Unreachable(format!(
                "the window at {} answered a moment ago and now cannot be reached ({error}), so \
                 nothing was sent. It is most likely quitting; run this again.",
                self.path
            ))
        })
    }
}

/// Why a window was not asked anything. Nothing was sent to it but the question every request
/// starts with, which changes nothing.
#[derive(Debug)]
enum Miss {
    /// Nothing is listening: the socket file is gone, or a window that was killed left it behind.
    Gone(std::io::Error),
    /// Something may be listening, and this process may not connect to it. EPERM is a sandbox
    /// refusing every socket; EACCES is the file's permissions.
    Forbidden(std::io::Error),
    /// It took the connection and said nothing within this long.
    Silent(Duration),
    /// It took the connection and hung up without answering, as a window that is quitting does.
    HungUp(String),
}

impl Miss {
    fn of(error: std::io::Error) -> Miss {
        if error.kind() == ErrorKind::PermissionDenied {
            Miss::Forbidden(error)
        } else {
            Miss::Gone(error)
        }
    }

    /// What happened at `path`, as a sentence of its own.
    fn said(&self, path: &str) -> String {
        match self {
            Miss::Gone(error) => format!("nothing is listening on {path} ({error})."),
            Miss::Forbidden(error) => not_permitted(&format!("the window at {path}"), error),
            Miss::Silent(waited) => format!(
                "the window at {path} took the connection and did not answer within {waited:?}, \
                 so nothing was sent to it. Its machine may be asleep - a laptop forwarding the \
                 window over ssh is the usual case - or the app is hung."
            ),
            Miss::HungUp(detail) => format!(
                "the window at {path} hung up without answering ({detail}), as a window that is \
                 quitting does."
            ),
        }
    }
}

/// Why a socket this process found may not be connected to, and what to do about it.
///
/// Shared with the daemon's socket (`daemon::connect_welcomed`), since a sandbox that refuses one
/// refuses both. Codex's workspace-write sandbox reports EPERM for every Unix socket, which read
/// as a window that had quit while it was open (kan a_2cW58Xs6E).
pub(crate) fn not_permitted(what: &str, error: &std::io::Error) -> String {
    format!(
        "{what} is there, but this process is not permitted to connect to it ({error}), so \
         nothing was sent. An agent's sandbox refusing local sockets is the usual cause - Codex's \
         workspace-write and read-only sandboxes refuse every one - and otherwise the socket's \
         permissions do not let this user in. Let the sandbox reach the network (for Codex, \
         `network_access = true` under `[sandbox_workspace_write]` in ~/.codex/config.toml; \
         `muster docs harnesses`), or run this outside the sandbox."
    )
}

/// Which window to ask, found by asking each what it is showing.
///
/// `--socket` names one, which is asked or refused. Otherwise the window `$MUSTER_SOCKET` names,
/// when it answers - a pane's own app is the one it means. Failing that, the windows [`around`]
/// finds, and of several the one holding the tab or pane the request is about ([`choose`]).
///
/// No window answering is [`Trouble::NoWindow`], which is the one failure this machine's daemon
/// may answer in place of, unless a socket refused this process outright: a sandbox that does
/// that refuses the daemon's socket too, and saying so once is the answer.
fn reach(
    socket: Option<&str>,
    environment: &BTreeMap<String, String>,
    request: &Request,
    probe: &Request,
    patience: Duration,
) -> Result<Live, Trouble> {
    let within = patience.min(ALIVE_WITHIN);
    if let Some(path) = socket {
        return probed(path, probe, within).map_err(|miss| {
            Trouble::Unreachable(format!(
                "{} That is where --socket said to look, so no other window was tried.",
                miss.said(path)
            ))
        });
    }

    let own = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
    let mut missed = Vec::new();
    if let Some(path) = own {
        match probed(path, probe, within) {
            Ok(live) => return Ok(live),
            Err(miss) => missed.push((path.clone(), miss)),
        }
    }
    let (live, around_missed) = probe_all(around(environment), probe, within);
    missed.extend(around_missed);
    if live.is_empty() {
        return Err(nobody(environment, own.map(String::as_str), &missed));
    }

    let any_will_do = request.payload.as_ref().is_some_and(muster_proto::any_window_will_do);
    let subject = subject(request);
    choose(live, subject, any_will_do).map_err(|live| several(&live, subject))
}

/// The window to ask among several that answered: the first holding what the request is about,
/// or with none holding it, the first, when any window can carry the request out.
///
/// Several answering are several apps - two installs, or two laptops forwarding windows to one
/// devenv - and an app carries a request to the window holding its tab only among its own. Two
/// that both hold it can each carry it out, so the first does.
fn choose(
    mut live: Vec<Live>,
    subject: Option<Names<'_>>,
    any_will_do: bool,
) -> Result<Live, Vec<Live>> {
    if live.len() == 1 {
        return Ok(live.remove(0));
    }
    let holder = subject.and_then(|subject| {
        live.iter().position(|live| live.window().is_some_and(|window| holds(window, subject)))
    });
    match holder {
        Some(holder) => Ok(live.remove(holder)),
        None if any_will_do => Ok(live.remove(0)),
        None => Err(live),
    }
}

/// Why several windows answering is a refusal.
fn several(live: &[Live], subject: Option<Names<'_>>) -> Trouble {
    let paths = live.iter().map(|live| live.path.as_str()).collect::<Vec<_>>().join(", ");
    let count = live.len();
    Trouble::Unreachable(match subject {
        Some(Names::Pane(pane)) => format!(
            "{count} Muster apps are listening and none of them holds pane {pane}: {paths}. \
             `muster window` run outside every pane lists what each holds; pick one with \
             --socket."
        ),
        Some(Names::Tab(tab)) => format!(
            "{count} Muster apps are listening and none of them holds tab {tab}: {paths}. \
             `muster window` run outside every pane lists what each holds; pick one with \
             --socket."
        ),
        None => format!(
            "{count} Muster apps are listening and nothing says which one this is about: \
             {paths}. Run this inside one of their panes, where ${} names it, or pick one with \
             --socket.",
            environment::WINDOW_SOCKET
        ),
    })
}

/// The tab or pane a request is about, when it names one: what a change names
/// (`muster_proto::names`), and the pane a read or a wait is about.
pub fn named(request: &Request) -> Option<Names<'_>> {
    let payload = request.payload.as_ref()?;
    muster_proto::names(payload).or_else(|| match payload {
        request::Payload::ReadPane(read) if !read.pane_id.is_empty() => {
            Some(Names::Pane(&read.pane_id))
        }
        request::Payload::WatchPanes(watch) => {
            watch.pane_ids.first().filter(|pane| !pane.is_empty()).map(|pane| Names::Pane(pane))
        }
        _ => None,
    })
}

/// What a request is about, for choosing the window holding it: what it names, or else the pane
/// it was sent from - which is what a pane's `pane new` splits and its `muster window` means.
fn subject(request: &Request) -> Option<Names<'_>> {
    named(request)
        .or_else(|| (!request.from_pane.is_empty()).then_some(Names::Pane(&request.from_pane)))
}

/// Whether a window's app holds a tab or pane, in this window or in any other of its windows,
/// open or closed: a closed window's tabs are still its, and its agents still running.
fn holds(window: &muster_proto::Window, subject: Names<'_>) -> bool {
    let mut tabs = window
        .roster
        .iter()
        .flat_map(|roster| &roster.tabs)
        .chain(window.windows.iter().flat_map(|other| &other.tabs));
    tabs.any(|tab| match subject {
        Names::Tab(id) => tab.tab_id == id,
        Names::Pane(id) => tab.panes.iter().any(|pane| pane.pane_id == id),
    })
}

/// Why no window answered, as [`Trouble::NoWindow`] - or, when a socket refused this process,
/// as a refusal no daemon is asked past.
fn nobody(
    environment: &BTreeMap<String, String>,
    own: Option<&str>,
    missed: &[(String, Miss)],
) -> Trouble {
    if let Some((path, miss)) = missed.iter().find(|(_, miss)| matches!(miss, Miss::Forbidden(_))) {
        return Trouble::Unreachable(miss.said(path));
    }
    let silent = silent(missed);
    let beside: Vec<&str> = missed
        .iter()
        .filter(|(path, miss)| Some(path.as_str()) != own && matches!(miss, Miss::Silent(_)))
        .map(|(path, _)| path.as_str())
        .collect();
    let also = if beside.is_empty() {
        String::new()
    } else {
        format!(
            " {} took the connection and did not answer within {ALIVE_WITHIN:?}, so nothing \
             was sent there either.",
            beside.join(", ")
        )
    };
    let pane = environment.get(environment::PANE_NAME).filter(|pane| !pane.is_empty());
    let socket = environment::WINDOW_SOCKET;
    let reason = match (own, missed.iter().find(|(path, _)| Some(path.as_str()) == own)) {
        (Some(path), Some((_, Miss::Gone(error)))) => format!(
            "nothing is listening on {path} ({error}), which is the window ${socket} names, and \
             no other window of Muster answered beside it.{also} That window has quit, and this \
             pane outlived it - the pane's daemon kept it running. Open Muster again, or name a \
             window with --socket."
        ),
        (Some(path), Some((_, miss))) => format!(
            "{} That is the window ${socket} names, and no other window of Muster answered \
             beside it.{also} Name a window with --socket to ask another.",
            miss.said(path)
        ),
        _ => {
            let state =
                state_directory(environment).unwrap_or_else(|| "~/.muster/state".to_string());
            match pane {
                Some(pane) => format!(
                    "no Muster window answered. ${socket} is not set in this pane ({pane}) - a \
                     pane restored after its daemon restarted has none, and so does a program \
                     that cleared its environment - so the windows under {state} and beside this \
                     machine's muster-daemon were asked, and none answered.{also} Open Muster, \
                     or name a window with --socket."
                ),
                None => format!(
                    "no Muster window is listening. ${socket} is not set, so this is not running \
                     in a pane Muster made, and nothing under {state} answered.{also} Open \
                     Muster, or name a window with --socket."
                ),
            }
        }
    };
    Trouble::NoWindow { reason, silent }
}

/// The windows that took the connection and did not answer in time.
fn silent(missed: &[(String, Miss)]) -> Vec<String> {
    missed
        .iter()
        .filter(|(_, miss)| matches!(miss, Miss::Silent(_)))
        .map(|(path, _)| path.clone())
        .collect()
}

/// What a command answered by this machine's daemon says about a window it passed over for not
/// answering, so a caller knows the answer is the daemon's and why.
pub fn passed_over(path: &str) -> String {
    format!(
        "the window at {path} took the connection and did not answer within {ALIVE_WITHIN:?} - \
         its machine may be asleep - so this machine's muster-daemon answered instead."
    )
}

/// Asks each window what it is showing, all at once, so that several sockets nobody answers on
/// cost [`ALIVE_WITHIN`] together rather than each. In the order given.
fn probe_all(
    windows: Vec<String>,
    probe: &Request,
    within: Duration,
) -> (Vec<Live>, Vec<(String, Miss)>) {
    let probed: Vec<(String, Result<Live, Miss>)> = std::thread::scope(|scope| {
        let asked: Vec<_> = windows
            .into_iter()
            .map(|path| {
                scope.spawn(move || {
                    let probed = probed(&path, probe, within);
                    (path, probed)
                })
            })
            .collect();
        asked.into_iter().filter_map(|asked| asked.join().ok()).collect()
    });
    let mut live = Vec::new();
    let mut missed = Vec::new();
    for (path, probed) in probed {
        match probed {
            Ok(answered) => live.push(answered),
            Err(miss) => missed.push((path, miss)),
        }
    }
    (live, missed)
}

/// Asks one window what it is showing, on a connection of its own.
fn probed(path: &str, probe: &Request, within: Duration) -> Result<Live, Miss> {
    let mut stream = dial(path).map_err(Miss::of)?;
    let _ = stream.set_read_timeout(Some(within));
    let _ = stream.set_write_timeout(Some(within));
    write_frame(&mut stream, &probe.encode_to_vec())
        .map_err(|error| Miss::HungUp(error.to_string()))?;
    let mut watched = Watched { stream: &mut stream, timed_out: false };
    match read_frame(&mut watched, LARGEST_MESSAGE) {
        Ok(reply) => Ok(Live { path: path.to_string(), answer: decoded(path, &reply) }),
        Err(_) if watched.timed_out => Err(Miss::Silent(within)),
        Err(detail) => Err(Miss::HungUp(detail)),
    }
}

/// A stream that remembers whether a read ran out of time, which the framing's error, a string,
/// no longer says.
struct Watched<'a> {
    stream: &'a mut UnixStream,
    timed_out: bool,
}

impl Read for Watched<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.stream.read(buffer);
        if let Err(error) = &read
            && matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
        {
            self.timed_out = true;
        }
        read
    }
}

/// Whether the window `$MUSTER_SOCKET` names is gone: set, and nothing answering there.
///
/// Only dialled, so a window that accepts and then says nothing is not gone by this: it is passed
/// over once asked ([`reach`]), and asking it here as well would cost the wait twice.
pub fn own_window_gone(environment: &BTreeMap<String, String>) -> bool {
    environment
        .get(environment::WINDOW_SOCKET)
        .is_some_and(|path| !path.is_empty() && dial(path).is_err())
}

/// The windows to ask when nobody named one, or the one named does not answer: beside a pane's
/// own window, or with none named, every window in Muster's state directory. In a pane, also the
/// windows forwarded to this machine's daemon, since on a devenv those are where a pane's windows
/// listen.
pub fn around(environment: &BTreeMap<String, String>) -> Vec<String> {
    let own = environment.get(environment::WINDOW_SOCKET).filter(|path| !path.is_empty());
    let mut found = match own {
        Some(path) => siblings(path),
        None => candidates(environment),
    };
    if environment.get(environment::PANE_NAME).is_some_and(|pane| !pane.is_empty()) {
        for path in beside_the_daemon(environment) {
            if Some(&path) != own && !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
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
    sockets_in(directory, &format!("{kind}-"), Some(own))
}

/// The windows forwarded over ssh to this machine's daemon: `window-<install>-*.sock` beside its
/// socket, the install being that socket's own name, as `window_beside` in muster-seam's
/// `session.rs` names them.
///
/// Found from the daemon rather than from `$MUSTER_SOCKET`, for the pane that has none: one
/// restored after its daemon restarted, or run under a program that cleared it (kan
/// a_2cW584sro).
fn beside_the_daemon(environment: &BTreeMap<String, String>) -> Vec<String> {
    let Some(socket) = crate::daemon::socket(environment) else { return Vec::new() };
    let (Some(directory), Some(install)) =
        (socket.parent(), socket.file_stem().and_then(|stem| stem.to_str()))
    else {
        return Vec::new();
    };
    sockets_in(directory, &format!("window-{install}-"), None)
}

/// The sockets in `directory` named `<prefix>*.sock`, but `except`, in a settled order.
///
/// Sorted so that a refusal naming several of them reads the same twice in a row - a directory
/// hands them back in whatever order it likes.
fn sockets_in(directory: &std::path::Path, prefix: &str, except: Option<&str>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            Some(name.as_ref()) != except && name.starts_with(prefix) && name.ends_with(".sock")
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
pub fn candidates(environment: &BTreeMap<String, String>) -> Vec<String> {
    let Some(state) = state_directory(environment) else { return Vec::new() };
    sockets_in(std::path::Path::new(&state), "command-", None)
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

    fn live(path: &str, panes: &[&str]) -> Live {
        let roster = muster_proto::RosterChanged {
            tabs: vec![muster_proto::RosterTab {
                tab_id: format!("t-{path}"),
                panes: panes
                    .iter()
                    .map(|pane| muster_proto::RosterPane {
                        pane_id: (*pane).to_string(),
                        ..muster_proto::RosterPane::default()
                    })
                    .collect(),
                ..muster_proto::RosterTab::default()
            }],
            ..muster_proto::RosterChanged::default()
        };
        let window =
            muster_proto::Window { roster: Some(roster), ..muster_proto::Window::default() };
        Live {
            path: path.to_string(),
            answer: Ok(Response { payload: Some(response::Payload::Window(window)) }),
        }
    }

    fn chosen(
        live: Vec<Live>,
        subject: Option<Names<'_>>,
        any_will_do: bool,
    ) -> Result<String, Vec<String>> {
        choose(live, subject, any_will_do)
            .map(|live| live.path)
            .map_err(|live| live.into_iter().map(|live| live.path).collect())
    }

    #[test]
    fn of_several_windows_the_one_holding_the_pane_is_asked() {
        let both = || vec![live("a", &["p1"]), live("b", &["p2"])];
        assert_eq!(chosen(both(), Some(Names::Pane("p2")), false), Ok("b".to_string()));
        assert_eq!(chosen(both(), Some(Names::Tab("t-a")), false), Ok("a".to_string()));
        assert_eq!(
            chosen(vec![live("a", &["p1"]), live("b", &["p1"])], Some(Names::Pane("p1")), false),
            Ok("a".to_string()),
            "two holding it can each carry it out"
        );
    }

    #[test]
    fn of_several_windows_none_holding_it_only_any_will_do_takes_the_first() {
        let both = || vec![live("a", &["p1"]), live("b", &["p2"])];
        assert_eq!(chosen(both(), Some(Names::Pane("p9")), true), Ok("a".to_string()));
        assert_eq!(
            chosen(both(), Some(Names::Pane("p9")), false),
            Err(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(chosen(both(), None, false), Err(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(chosen(vec![live("a", &[])], None, false), Ok("a".to_string()));
    }

    #[test]
    fn a_closed_windows_tabs_are_still_its_apps() {
        let mut window = muster_proto::Window::default();
        window.windows.push(muster_proto::OtherWindow {
            name: "window-2".to_string(),
            pid: 0,
            tabs: vec![muster_proto::RosterTab {
                tab_id: "t2".to_string(),
                panes: vec![muster_proto::RosterPane {
                    pane_id: "p2".to_string(),
                    ..muster_proto::RosterPane::default()
                }],
                ..muster_proto::RosterTab::default()
            }],
        });
        assert!(holds(&window, Names::Pane("p2")));
        assert!(holds(&window, Names::Tab("t2")));
        assert!(!holds(&window, Names::Pane("p1")));
    }

    #[test]
    fn a_request_is_about_what_it_names_or_else_the_pane_it_came_from() {
        use muster_proto::{ClosePane, ReadPane, WatchPanes};
        let mut close = Request::new(request::Payload::ClosePane(ClosePane::default()));
        assert_eq!(subject(&close), None);
        close.from_pane = "p-here".to_string();
        assert_eq!(subject(&close), Some(Names::Pane("p-here")));
        let read = Request::new(request::Payload::ReadPane(ReadPane {
            pane_id: "p1".to_string(),
            ..ReadPane::default()
        }));
        assert_eq!(named(&read), Some(Names::Pane("p1")));
        let wait = Request::new(request::Payload::WatchPanes(WatchPanes {
            pane_ids: vec!["p2".to_string(), "p3".to_string()],
            ..WatchPanes::default()
        }));
        assert_eq!(named(&wait), Some(Names::Pane("p2")));
    }

    /// EPERM is a sandbox and EACCES a file's permissions; both are a socket this process may not
    /// open, and neither is a window that has gone.
    #[test]
    fn a_permission_error_is_not_a_window_gone() {
        // The same numbers on macOS and Linux, unlike ECONNREFUSED's.
        let (eperm, eacces, enoent) = (1, 13, 2);
        let os = std::io::Error::from_raw_os_error;
        assert!(matches!(Miss::of(os(eperm)), Miss::Forbidden(_)), "EPERM");
        assert!(matches!(Miss::of(os(eacces)), Miss::Forbidden(_)), "EACCES");
        assert!(matches!(Miss::of(os(enoent)), Miss::Gone(_)), "ENOENT");
        let refused = std::io::Error::from(ErrorKind::ConnectionRefused);
        assert!(matches!(Miss::of(refused), Miss::Gone(_)), "ECONNREFUSED");
        assert!(Miss::of(os(eperm)).said("/w.sock").contains("sandbox"));
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

    /// A window older than `turn` reads the newest rows instead, and says nothing about it: that
    /// answer is refused rather than printed as the agent's report.
    #[test]
    fn a_turn_read_answered_with_the_newest_rows_is_refused() {
        use muster_proto::{PaneText, ReadPane, response};
        let read = |turn| {
            Request::new(request::Payload::ReadPane(ReadPane { turn, ..ReadPane::default() }))
        };
        let text = |turn| Response {
            payload: Some(response::Payload::PaneText(PaneText { turn, ..PaneText::default() })),
        };
        assert!(as_asked("w", &read(true), text(true)).is_ok());
        assert!(as_asked("w", &read(false), text(false)).is_ok());
        let refused = as_asked("w", &read(true), text(false)).unwrap_err();
        assert!(
            matches!(&refused, Trouble::Refused(why) if why.contains("predates")),
            "{refused:?}"
        );
    }
}
