//! A window's questions about panes, answered by this machine's daemon when no window does.
//!
//! What agents are doing, what a pane printed, typing into one, and waiting on one are all the
//! daemon's to know: a window only relays them. So on a devenv with no window reachable, or in a
//! pane whose window has quit, these still work, against the daemon that holds the panes. The
//! answers are the window's own messages, built from the daemon's, so the same renderer prints
//! them and a caller reads the same thing either way. What only a window has - tabs arranged in
//! regions, places, the keyboard, what is on screen - is left out rather than made up.
//!
//! Every rule the window applies on the way is shared rather than copied: paging to a pane's
//! newest rows (`muster_daemon_proto::pane_text`), counting rows and confirming a send
//! (`muster_core::pane_text`), and which states a wait accepts (`muster_core::AgentState`).

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use muster_core::AgentState;
use muster_core::pane_text::{self, PaneText};
use muster_daemon_proto::{self as daemon_proto, ConnectionKind, connection};
use muster_proto::{Request, Response, request, response};

use crate::dial::Ended;
use crate::{Trouble, daemon};

/// How long a request to the daemon may take to be answered.
const PATIENCE: Duration = Duration::from_mins(1);

/// Whether this is a request the daemon can answer without a window.
pub fn can_answer(request: &Request) -> bool {
    matches!(
        request.payload,
        Some(
            request::Payload::ReadWindow(_)
                | request::Payload::ReadPane(_)
                | request::Payload::SendToPane(_)
                | request::Payload::WatchPanes(_)
        )
    )
}

/// What a request not answered by a window came to.
#[derive(Debug)]
pub enum Answered {
    /// The window's own answer, for the renderer every window answer goes through.
    Response(Box<Response>),
    /// What the daemon holds, which only a window could lay out: printed by
    /// [`crate::render::daemon_window`].
    Window { window: Box<muster_proto::Window>, socket: String },
}

/// Answers `request` from this machine's daemon.
pub fn ask(request: &Request, environment: &BTreeMap<String, String>) -> Result<Answered, Trouble> {
    let socket = daemon::socket_or_refusal(environment)?;
    let respond = |response| Ok(Answered::Response(Box::new(response)));
    match &request.payload {
        Some(request::Payload::ReadWindow(_)) => {
            let snapshot = snapshot(&socket)?;
            Ok(Answered::Window {
                window: Box::new(window_of(&snapshot, &socket)),
                socket: socket.display().to_string(),
            })
        }
        Some(request::Payload::ReadPane(read)) => {
            let pane = named(&read.pane_id, "read")?;
            respond(match read_pane(&socket, pane, read.rows) {
                Ok(read) => Response {
                    payload: Some(response::Payload::PaneText(muster_proto::PaneText {
                        rows: u32::try_from(pane_text::rows_of(&read.text).len())
                            .unwrap_or(u32::MAX),
                        text: read.text,
                        truncated: read.truncated,
                    })),
                },
                Err(refusal) => failure(refusal),
            })
        }
        Some(request::Payload::SendToPane(send)) => {
            let pane = named(&send.pane_id, "sent")?;
            respond(send_to_pane(&socket, pane, send))
        }
        _ => Err(Trouble::Refused(
            "this asks something only a window can answer, and no window answered. That is a bug \
             in muster: nothing else should have been sent here."
                .to_string(),
        )),
    }
}

/// A pane's name, or a refusal for the pane a window's keyboard is on, which there is none of.
fn named<'a>(pane: &'a str, done: &str) -> Result<&'a str, Trouble> {
    if pane.is_empty() {
        return Err(Trouble::Refused(format!(
            "no window answered, so no pane has a keyboard for this to mean, and nothing was \
             {done}. Name the pane with --pane; `muster window` lists them."
        )));
    }
    Ok(pane)
}

fn failure(reason: String) -> Response {
    Response { payload: Some(response::Payload::Failure(muster_proto::Failure { reason })) }
}

// ---------------------------------------------------------------------------------------------
// Asking the daemon

/// One request on a control connection of its own, and its answer.
fn asked(
    socket: &Path,
    service: daemon_proto::request::Service,
) -> Result<daemon_proto::Answer, Trouble> {
    let mut stream = daemon::connect(socket, ConnectionKind::Control)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    connection::send(&mut stream, &daemon_proto::Request { id: 1, service: Some(service) })
        .map_err(|error| Trouble::Unreachable(format!("{}: {error}", socket.display())))?;
    loop {
        match connection::receive::<daemon_proto::ControlMessage>(&mut stream) {
            Ok(Some(daemon_proto::ControlMessage {
                message: Some(daemon_proto::control_message::Message::Answer(answer)),
            })) => return Ok(answer),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                return Err(Trouble::Unanswered(format!(
                    "the muster-daemon at {} hung up before answering.",
                    socket.display()
                )));
            }
        }
    }
}

fn snapshot(socket: &Path) -> Result<daemon_proto::Snapshot, Trouble> {
    let asked = asked(
        socket,
        daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(daemon_proto::session_request::Request::Snapshot(
                daemon_proto::session_request::Snapshot {},
            )),
        }),
    )?;
    match asked.detail {
        Some(daemon_proto::answer::Detail::Snapshot(snapshot)) => Ok(snapshot),
        _ => Err(Trouble::Refused(format!(
            "the muster-daemon at {} answered a snapshot with none ({}). That is a bug in the \
             daemon.",
            socket.display(),
            asked.reason
        ))),
    }
}

/// A pane's last `rows` rows, or everything for zero, read and cut the way a window does.
fn read_pane(socket: &Path, pane: &str, rows: u32) -> Result<PaneText, String> {
    let newest = daemon_proto::pane_text::newest(rows, |first_row, last| {
        let answer = asked(
            socket,
            daemon_proto::request::Service::Pane(daemon_proto::PaneRequest {
                request: Some(daemon_proto::pane_request::Request::Read(
                    daemon_proto::pane_request::Read {
                        pane: pane.to_string(),
                        first_row,
                        rows: 0,
                        last,
                    },
                )),
            }),
        )
        .map_err(|trouble| trouble.detail().to_string())?;
        match answer.detail {
            Some(daemon_proto::answer::Detail::Text(text)) => Ok(text),
            _ if answer.outcome() == daemon_proto::Outcome::NotThere => Err(format!(
                "the muster-daemon at {} holds no pane called {pane}. Either it closed, or the \
                 name is from another machine - `muster window` lists the panes this one has.",
                socket.display()
            )),
            _ => Err(format!(
                "the muster-daemon at {} would not read pane {pane}: {}",
                socket.display(),
                answer.reason
            )),
        }
    })?;
    Ok(PaneText { text: newest.text, truncated: newest.truncated }.tail(rows))
}

/// Types into a pane on the daemon's input connection, which answers nothing, then confirms it
/// by reading back when asked to - as the window does, by the same rule.
fn send_to_pane(socket: &Path, pane: &str, send: &muster_proto::SendToPane) -> Response {
    // The input connection says nothing about a pane it does not have, so a name nobody holds
    // is refused here, as the window refuses it, rather than reported sent.
    match snapshot(socket) {
        Ok(snapshot) if snapshot.panes.iter().any(|held| held.pane == pane) => {}
        Ok(_) => {
            return failure(format!(
                "the muster-daemon at {} holds no pane called {pane}, so nothing was sent. \
                 `muster window` lists the panes this machine has.",
                socket.display()
            ));
        }
        Err(trouble) => return failure(trouble.detail().to_string()),
    }
    let typed = daemon::connect(socket, ConnectionKind::Input).and_then(|mut stream| {
        let event = daemon_proto::InputEvent {
            pane: pane.to_string(),
            input: Some(daemon_proto::input_event::Input::Send(daemon_proto::input_event::Send {
                text: send.text.clone(),
                enter: send.enter,
            })),
        };
        connection::send(&mut stream, &event).map_err(|error| {
            Trouble::Unreachable(format!(
                "nothing was sent to {pane}: the muster-daemon at {} would not take it ({error}).",
                socket.display()
            ))
        })
    });
    if let Err(trouble) = typed {
        return failure(trouble.detail().to_string());
    }
    if !send.confirm {
        return Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) };
    }
    match pane_text::confirm(pane, &send.text, |rows| {
        read_pane(socket, pane, rows).map(|read| read.text)
    }) {
        Ok(()) => Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) },
        Err(refusal) => failure(refusal),
    }
}

// ---------------------------------------------------------------------------------------------
// What the daemon holds, as a window would describe it

/// The word a window paints for a pane, from the daemon's record alone.
///
/// The window's rule (`Pane::presented_state`, then `Attention::presented`): an idle agent
/// that said what it is waiting on is `waiting`, and a finish nobody has seen is `done` unless
/// the agent is busy again. A window also clears `done` for a pane it shows with the keyboard;
/// with no window, nobody has.
pub fn state_of(pane: &daemon_proto::Pane) -> AgentState {
    let waiting = pane.facts.as_ref().is_some_and(|facts| facts.waiting.is_some());
    let state = match pane.agent_state() {
        daemon_proto::AgentState::Working => AgentState::Working,
        daemon_proto::AgentState::Blocked => AgentState::Blocked,
        daemon_proto::AgentState::Idle if waiting => AgentState::Waiting,
        daemon_proto::AgentState::Idle => AgentState::Idle,
        daemon_proto::AgentState::Unknown => AgentState::Unknown,
    };
    let busy = matches!(state, AgentState::Working | AgentState::Blocked | AgentState::Waiting);
    if !busy && pane.finished_unseen { AgentState::Done } else { state }
}

fn pane_state(daemon_id: &str, pane: &daemon_proto::Pane) -> muster_proto::PaneStateChanged {
    let facts = pane.facts.clone().unwrap_or_default();
    let said = facts != daemon_proto::AgentFacts::default();
    muster_proto::PaneStateChanged {
        daemon_id: daemon_id.to_string(),
        pane_id: pane.pane.clone(),
        state: state_of(pane).as_str().to_string(),
        // The daemon does not say since when, and a window says when it first saw a pane.
        since_ms: 0,
        reported: pane.state_reported,
        unreadable: pane.screen_unreadable,
        facts: said.then(|| muster_proto::AgentFacts {
            context_used: facts.context_used,
            subagents: facts.subagents,
            model: facts.model.unwrap_or_default(),
            cost_usd: facts.cost_usd,
            waiting: facts.waiting.unwrap_or_default(),
            other: facts.other.into_iter().collect(),
        }),
        progress: None,
        rang: false,
    }
}

/// What a machine is called when no window has named it: its host name.
fn this_machine() -> String {
    let mut name = [0u8; 256];
    // SAFETY: the buffer is valid for its length, and gethostname writes at most that many
    // bytes into it.
    let named = unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } == 0;
    let end = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
    let host = String::from_utf8_lossy(&name[..end]).into_owned();
    if named && !host.is_empty() { host } else { "this-machine".to_string() }
}

/// Every tab and pane the daemon holds, in the shape of a window's answer.
fn window_of(snapshot: &daemon_proto::Snapshot, socket: &Path) -> muster_proto::Window {
    let machine = this_machine();
    let records: BTreeMap<&str, &daemon_proto::Pane> =
        snapshot.panes.iter().map(|pane| (pane.pane.as_str(), pane)).collect();
    let tabs = snapshot
        .tabs
        .iter()
        .map(|tab| {
            let mut panes = Vec::new();
            panes_of(tab.root.as_ref(), &mut panes);
            muster_proto::RosterTab {
                daemon_ids: vec![machine.clone()],
                tab_id: tab.tab.clone(),
                label: tab.label.as_ref().and_then(|label| label.text.clone()).unwrap_or_default(),
                given_name: tab
                    .label
                    .as_ref()
                    .and_then(|label| label.text.clone())
                    .unwrap_or_default(),
                panes: panes
                    .into_iter()
                    .map(|pane| {
                        let record = records.get(pane.as_str());
                        let label = record.and_then(|record| record.label.clone());
                        muster_proto::RosterPane {
                            daemon_id: machine.clone(),
                            label: label.clone().unwrap_or_else(|| {
                                record
                                    .map_or_else(String::new, |record| directory_name(&record.cwd))
                            }),
                            given_name: label.unwrap_or_default(),
                            subtitle: record
                                .map_or_else(String::new, |record| record.title.clone()),
                            pane_id: pane,
                            ..muster_proto::RosterPane::default()
                        }
                    })
                    .collect(),
                ..muster_proto::RosterTab::default()
            }
        })
        .collect();
    let directories: BTreeSet<String> =
        snapshot.panes.iter().map(|pane| pane.cwd.clone()).filter(|cwd| !cwd.is_empty()).collect();
    muster_proto::Window {
        roster: Some(muster_proto::RosterChanged {
            tabs,
            ..muster_proto::RosterChanged::default()
        }),
        panes: snapshot.panes.iter().map(|pane| pane_state(&machine, pane)).collect(),
        daemons: vec![muster_proto::Machine {
            daemon_id: machine,
            socket: socket.display().to_string(),
            panes: u32::try_from(snapshot.panes.len()).unwrap_or(u32::MAX),
            directories: directories.into_iter().collect(),
            state: "connected".to_string(),
            ..muster_proto::Machine::default()
        }],
        ..muster_proto::Window::default()
    }
}

/// A tab's panes, first to last as its tree lays them out.
fn panes_of(node: Option<&daemon_proto::Node>, into: &mut Vec<String>) {
    match node.and_then(|node| node.node.as_ref()) {
        Some(daemon_proto::node::Node::Pane(pane)) => into.push(pane.clone()),
        Some(daemon_proto::node::Node::Split(split)) => {
            panes_of(split.first.as_deref(), into);
            panes_of(split.second.as_deref(), into);
        }
        None => {}
    }
}

/// The last part of a directory, which is what a window calls a pane nobody named.
fn directory_name(cwd: &str) -> String {
    cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------------------------
// Watching

/// A watch on the daemon's panes, answering as a window's watch does (`muster-seam`'s
/// `watch.rs`): a condition rather than an event, `idle` met by `done`, a named pane that closes
/// ending a wait with a failure, and with no panes named, every pane including new ones.
#[derive(Debug)]
pub struct Watching {
    stream: UnixStream,
    socket: PathBuf,
    machine: String,
    panes: Option<BTreeSet<String>>,
    until: Vec<AgentState>,
    /// What was last said about each pane, so a record that changed in some other way is not
    /// said again.
    sent: BTreeMap<String, AgentState>,
    ready: std::collections::VecDeque<Response>,
    ended: bool,
}

/// Starts watching, or says why it cannot, before anything is watched.
pub fn follow(
    request: &Request,
    environment: &BTreeMap<String, String>,
) -> Result<Watching, Trouble> {
    let Some(request::Payload::WatchPanes(watch)) = &request.payload else {
        return Err(Trouble::Refused(
            "only a watch on panes follows the daemon; this is a bug in muster.".to_string(),
        ));
    };
    let mut until = Vec::new();
    for word in &watch.until {
        let Some(state) = AgentState::ALL.into_iter().find(|state| state.as_str() == word) else {
            let states: Vec<_> = AgentState::ALL.iter().map(|state| state.as_str()).collect();
            return Err(Trouble::Refused(format!(
                "`{word}` is not a state a pane can be in, so there is nothing to wait for. The \
                 states are {}.",
                states.join(", ")
            )));
        };
        until.push(state);
    }
    let socket = daemon::socket_or_refusal(environment)?;
    let mut stream = daemon::connect(&socket, ConnectionKind::Control)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let subscribe = daemon_proto::Request {
        id: 1,
        service: Some(daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(daemon_proto::session_request::Request::Subscribe(
                daemon_proto::session_request::Subscribe {},
            )),
        })),
    };
    connection::send(&mut stream, &subscribe)
        .map_err(|error| Trouble::Unreachable(format!("{}: {error}", socket.display())))?;
    let snapshot = loop {
        match connection::receive::<daemon_proto::ControlMessage>(&mut stream) {
            Ok(Some(daemon_proto::ControlMessage {
                message: Some(daemon_proto::control_message::Message::Answer(answer)),
            })) => match answer.detail {
                Some(daemon_proto::answer::Detail::Snapshot(snapshot)) => break snapshot,
                _ => {
                    return Err(Trouble::Refused(format!(
                        "the muster-daemon at {} would not be watched: {}",
                        socket.display(),
                        answer.reason
                    )));
                }
            },
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                return Err(Trouble::Unreachable(format!(
                    "the muster-daemon at {} hung up before the watch began.",
                    socket.display()
                )));
            }
        }
    };
    let panes = if watch.pane_ids.is_empty() {
        None
    } else {
        for named in &watch.pane_ids {
            if !snapshot.panes.iter().any(|pane| &pane.pane == named) {
                return Err(Trouble::Refused(format!(
                    "the muster-daemon at {} holds no pane called {named}, so there is nothing \
                     to watch. Either it closed, or the name is from another machine - `muster \
                     window` lists the panes this one has.",
                    socket.display()
                )));
            }
        }
        Some(watch.pane_ids.iter().cloned().collect())
    };
    let mut watching = Watching {
        stream,
        socket,
        machine: this_machine(),
        panes,
        until,
        sent: BTreeMap::new(),
        ready: std::collections::VecDeque::new(),
        ended: false,
    };
    watching.begin(&snapshot.panes);
    Ok(watching)
}

impl Watching {
    /// The first picture: everything, for a watch with nothing to wait for; for a wait, the
    /// panes already there and the end, if any are.
    fn begin(&mut self, panes: &[daemon_proto::Pane]) {
        let watched: Vec<&daemon_proto::Pane> =
            panes.iter().filter(|pane| self.watches(&pane.pane)).collect();
        for pane in &watched {
            self.sent.insert(pane.pane.clone(), state_of(pane));
        }
        if self.until.is_empty() {
            for pane in watched {
                self.ready.push_back(self.said(pane));
            }
            return;
        }
        let arrived: Vec<&daemon_proto::Pane> =
            watched.into_iter().filter(|pane| self.arrived(state_of(pane))).collect();
        if !arrived.is_empty() {
            for pane in arrived {
                self.ready.push_back(self.said(pane));
            }
            self.last(Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) });
        }
    }

    fn said(&self, pane: &daemon_proto::Pane) -> Response {
        Response { payload: Some(response::Payload::PaneState(pane_state(&self.machine, pane))) }
    }

    fn last(&mut self, response: Response) {
        self.ready.push_back(response);
        self.ended = true;
    }

    fn watches(&self, pane: &str) -> bool {
        self.panes.as_ref().is_none_or(|panes| panes.contains(pane))
    }

    fn arrived(&self, state: AgentState) -> bool {
        self.until.iter().any(|wanted| state.counts_as(*wanted))
    }

    fn changed(&mut self, pane: &daemon_proto::Pane) {
        if self.ended || !self.watches(&pane.pane) {
            return;
        }
        let state = state_of(pane);
        if self.sent.insert(pane.pane.clone(), state) == Some(state) {
            return;
        }
        if self.until.is_empty() {
            self.ready.push_back(self.said(pane));
        } else if self.arrived(state) {
            self.ready.push_back(self.said(pane));
            self.last(Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) });
        }
    }

    fn closed(&mut self, pane: &str) {
        if self.ended {
            return;
        }
        let named = self.panes.as_ref().is_some_and(|panes| panes.contains(pane));
        let was_sent = self.sent.remove(pane).is_some();
        if self.until.is_empty() {
            if named || was_sent {
                self.ready.push_back(Response {
                    payload: Some(response::Payload::PaneClosed(muster_proto::PaneClosed {
                        daemon_id: self.machine.clone(),
                        pane_id: pane.to_string(),
                    })),
                });
            }
            return;
        }
        if named {
            let until: Vec<&str> = self.until.iter().map(|state| state.as_str()).collect();
            self.last(failure(format!(
                "pane {pane} closed before it was {}, so there is nothing left to wait for. \
                 Whatever was running in it ended, or somebody closed it; `muster window` lists \
                 what this machine still holds.",
                until.join(" or ")
            )));
        }
    }

    /// The next answer, waiting until `deadline` for it, or forever with none.
    pub fn next(&mut self, deadline: Option<Instant>) -> Result<Response, Ended> {
        loop {
            if let Some(response) = self.ready.pop_front() {
                return Ok(response);
            }
            let wait = deadline.map(|deadline| {
                deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))
            });
            let _ = self.stream.set_read_timeout(wait);
            let message = connection::receive::<daemon_proto::ControlMessage>(&mut self.stream);
            match message {
                Ok(Some(daemon_proto::ControlMessage {
                    message: Some(daemon_proto::control_message::Message::Event(event)),
                })) => match event.event {
                    Some(daemon_proto::event::Event::PaneOpened(opened)) => {
                        if let Some(pane) = opened.pane {
                            self.changed(&pane);
                        }
                    }
                    Some(daemon_proto::event::Event::PaneChanged(changed)) => {
                        if let Some(pane) = changed.pane {
                            self.changed(&pane);
                        }
                    }
                    Some(daemon_proto::event::Event::PaneClosed(closed)) => {
                        self.closed(&closed.pane);
                    }
                    _ => {}
                },
                Ok(Some(_)) => {}
                Err(_) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                    return Err(Ended::TimedOut);
                }
                Ok(None) | Err(_) => return Err(self.hung_up()),
            }
        }
    }

    fn hung_up(&self) -> Ended {
        Ended::HungUp(format!(
            "the muster-daemon at {} hung up in the middle of the watch, which is what a daemon \
             that stops or hands over to a new one does. Watching changed nothing, so run it \
             again.",
            self.socket.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(state: daemon_proto::AgentState, finished: bool, waiting: bool) -> daemon_proto::Pane {
        let mut pane = daemon_proto::Pane { finished_unseen: finished, ..Default::default() };
        pane.set_agent_state(state);
        if waiting {
            pane.facts = Some(daemon_proto::AgentFacts {
                waiting: Some("the gate".to_string()),
                ..Default::default()
            });
        }
        pane
    }

    /// The words a window paints, from a daemon's record with nobody having looked at it.
    #[test]
    fn a_pane_reads_as_a_window_would_paint_it() {
        use daemon_proto::AgentState as Daemon;
        let cases = [
            (Daemon::Working, false, false, "working"),
            (Daemon::Blocked, false, false, "blocked"),
            (Daemon::Idle, false, false, "idle"),
            (Daemon::Unknown, false, false, "unknown"),
            (Daemon::Idle, false, true, "waiting"),
            (Daemon::Idle, true, false, "done"),
            (Daemon::Unknown, true, false, "done"),
            // Busy again wins over a finish still recorded, and so does waiting.
            (Daemon::Working, true, false, "working"),
            (Daemon::Idle, true, true, "waiting"),
        ];
        for (daemon, finished, waiting, painted) in cases {
            assert_eq!(
                state_of(&pane(daemon, finished, waiting)).as_str(),
                painted,
                "{daemon:?}, finished unseen {finished}, waiting {waiting}"
            );
        }
    }
}
