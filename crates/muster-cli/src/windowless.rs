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

use muster_core::composition::{Rect, ViewNode, ViewPane, places_in};
use muster_core::harnesses;
use muster_core::mirror::backend::{Adapter, PaneId, SplitAxis};
use muster_core::names::{Mint, Minter};
use muster_core::pane_address::{self, Address, Candidate};
use muster_core::pane_text::{self, PaneText, Scope};
use muster_core::{AgentState, Until};
use muster_daemon_proto::{self as daemon_proto, ConnectionKind, connection};
use muster_proto::{Request, Response, request, response};

use crate::dial::Ended;
use crate::{Trouble, daemon, environment};

/// How long a request to the daemon may take to be answered.
const PATIENCE: Duration = Duration::from_mins(1);

/// Whether this is a request the daemon can answer without a window.
///
/// A pane is made here only on this machine: which machine a window's `--daemon` name means is
/// the window's to know.
pub fn can_answer(request: &Request) -> bool {
    match &request.payload {
        Some(request::Payload::SplitPane(split)) => {
            split.daemon_id.is_empty() && split.new_pane_daemon_id.is_empty()
        }
        payload => matches!(
            payload,
            Some(
                request::Payload::ReadWindow(_)
                    | request::Payload::ReadPane(_)
                    | request::Payload::SendToPane(_)
                    | request::Payload::WatchPanes(_)
                    | request::Payload::CompactPane(_)
                    | request::Payload::HidePane(_)
                    | request::Payload::ShowPane(_)
                    | request::Payload::QuietPane(_)
                    | request::Payload::RenamePane(_)
                    | request::Payload::ClosePane(_)
            )
        ),
    }
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
    let mut request = request.clone();
    if let Some(payload) = request.payload.as_mut() {
        let mut typed = muster_proto::typed_panes(payload);
        if typed.iter().any(|pane| !pane.is_empty()) {
            addressed(&mut typed, &snapshot(&socket)?)?;
        }
    }
    let respond = |response| Ok(Answered::Response(Box::new(response)));
    match &request.payload {
        Some(request::Payload::ReadWindow(read)) => {
            let snapshot = snapshot(&socket)?;
            let mut window = window_of(&snapshot, &socket);
            if read.layout {
                lay_out(&mut window, &snapshot, &socket);
            }
            Ok(Answered::Window { window: Box::new(window), socket: socket.display().to_string() })
        }
        Some(request::Payload::ReadPane(read)) => {
            let pane = named(&read.pane_id, "read")?;
            let scope = if read.turn { Scope::Turn } else { Scope::Newest(read.rows) };
            respond(match read_pane(&socket, pane, scope) {
                Ok(read) => Response {
                    payload: Some(response::Payload::PaneText(muster_proto::PaneText {
                        rows: u32::try_from(pane_text::rows_of(&read.text).len())
                            .unwrap_or(u32::MAX),
                        text: read.text,
                        truncated: read.truncated,
                        turn: read.turn,
                    })),
                },
                Err(refusal) => failure(refusal),
            })
        }
        Some(request::Payload::SendToPane(send)) => {
            let pane = named(&send.pane_id, "sent")?;
            respond(send_to_pane(&socket, pane, send))
        }
        Some(request::Payload::CompactPane(compact)) => {
            let pane = named(&compact.pane_id, "compacted")?;
            respond(compact_pane(&socket, pane, &compact.focus)?)
        }
        Some(request::Payload::HidePane(hide)) => {
            let pane = named(&hide.pane_id, "hidden")?;
            respond(move_hiding(&socket, pane, true)?)
        }
        // With no window there is no keyboard to move, so `take_focus` asks for nothing here: the
        // pane is shown, and joins the next window that opens.
        Some(request::Payload::ShowPane(show)) => {
            let pane = named(&show.pane_id, "shown")?;
            respond(move_hiding(&socket, pane, false)?)
        }
        Some(request::Payload::QuietPane(quiet)) => {
            let pane = named(&quiet.pane_id, "made quiet")?;
            respond(quiet_pane(&socket, pane, quiet.quiet)?)
        }
        Some(request::Payload::RenamePane(rename)) => {
            let pane = named(&rename.pane_id, "renamed")?;
            respond(rename_pane(&socket, pane, &rename.name)?)
        }
        Some(request::Payload::ClosePane(close)) => {
            let pane = named(&close.pane_id, "closed")?;
            respond(close_pane(&socket, pane)?)
        }
        Some(request::Payload::SplitPane(split)) => {
            respond(make_pane(&socket, split, environment)?)
        }
        _ => Err(Trouble::Refused(
            "this asks something only a window can answer, and no window answered. That is a bug \
             in muster: nothing else should have been sent here."
                .to_string(),
        )),
    }
}

/// Puts each pane's id in place of a name typed for it (`otter`), among the panes this daemon
/// holds, the way a window does (`muster_core::pane_address`). The machine in `otter@<machine>`
/// is this one's host name, which is what `muster window` says with no window.
fn addressed(typed: &mut [&mut String], snapshot: &daemon_proto::Snapshot) -> Result<(), Trouble> {
    let machine = this_machine();
    let candidates: Vec<Candidate<'_>> = snapshot
        .panes
        .iter()
        .map(|pane| Candidate { pane: &pane.pane, name: pane.name(), machine: &machine })
        .collect();
    for pane in typed.iter_mut() {
        match pane_address::resolve(pane, &candidates) {
            Address::Pane(id) => **pane = id,
            Address::AsTyped => {}
            Address::Ambiguous(why) => return Err(Trouble::Refused(why)),
        }
    }
    Ok(())
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

fn ok() -> Response {
    Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) }
}

/// The refusal for a pane this daemon does not hold, saying what was not `done` to it.
fn not_held(socket: &Path, pane: &str, done: &str) -> Response {
    failure(format!(
        "the muster-daemon at {} holds no pane called {pane}, so nothing was {done}. `muster \
         window` lists the panes this machine has.",
        socket.display()
    ))
}

fn pane_service(request: daemon_proto::pane_request::Request) -> daemon_proto::request::Service {
    daemon_proto::request::Service::Pane(daemon_proto::PaneRequest { request: Some(request) })
}

// ---------------------------------------------------------------------------------------------
// Asking the daemon

/// One request on a control connection of its own, and its answer.
fn asked(
    socket: &Path,
    service: daemon_proto::request::Service,
) -> Result<daemon_proto::Answer, Trouble> {
    let stream = daemon::connect(socket, ConnectionKind::Control)?;
    answered(stream, socket, service)
}

/// [`asked`], of a daemon that keeps hidden tabs and quiet panes, refusing `what` for an older
/// one, which would drop both without a word.
fn asked_of_one_that_hides(
    socket: &Path,
    service: daemon_proto::request::Service,
    what: &str,
) -> Result<daemon_proto::Answer, Trouble> {
    let (stream, welcome) = daemon::connect_welcomed(socket, ConnectionKind::Control)?;
    let speaks = welcome.protocol.unwrap_or_default();
    if speaks.minor < daemon_proto::version::HIDDEN_AND_QUIET {
        return Err(Trouble::Refused(format!(
            "this machine's muster-daemon speaks protocol {speaks}, which cannot {what}, so \
             nothing was changed. Update Muster; a new daemon takes over from an old one when \
             the app starts."
        )));
    }
    answered(stream, socket, service)
}

/// Sends one request on `stream` and waits for its answer.
fn answered(
    mut stream: UnixStream,
    socket: &Path,
    service: daemon_proto::request::Service,
) -> Result<daemon_proto::Answer, Trouble> {
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

/// A pane's last rows or its agent's last turn, read and cut the way a window does.
fn read_pane(socket: &Path, pane: &str, scope: Scope) -> Result<PaneText, String> {
    match scope {
        Scope::Newest(rows) => {
            let newest = daemon_proto::pane_text::newest(rows, |first_row, last| {
                read_page(socket, pane, first_row, last, false)
            })?;
            Ok(PaneText { text: newest.text, truncated: newest.truncated, turn: false }.tail(rows))
        }
        Scope::Turn => match daemon_proto::pane_text::turn(|| read_page(socket, pane, 0, 0, true))?
        {
            Some(turn) => {
                Ok(PaneText { text: turn.text, truncated: turn.truncated, turn: true }.tail(0))
            }
            None => Err(format!(
                "the muster-daemon at {} predates reading what a pane's agent printed in its \
                 last turn, and read something else; read the newest rows with --rows instead",
                socket.display()
            )),
        },
    }
}

/// One page of a pane's text from the daemon, as [`daemon_proto::pane_request::Read`] asks.
fn read_page(
    socket: &Path,
    pane: &str,
    first_row: u64,
    last: u32,
    turn: bool,
) -> Result<daemon_proto::PaneText, String> {
    let answer = asked(
        socket,
        daemon_proto::request::Service::Pane(daemon_proto::PaneRequest {
            request: Some(daemon_proto::pane_request::Request::Read(
                daemon_proto::pane_request::Read {
                    pane: pane.to_string(),
                    first_row,
                    rows: 0,
                    last,
                    turn,
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
}

/// Types into a pane on the daemon's input connection, which answers nothing, then confirms it
/// by reading back when asked to - as the window does, by the same rule.
///
/// What goes on the connection is the request's own text, keys and Return, untouched: the window
/// hands its daemon the same three, so a harness cannot tell which of the two sent it.
fn send_to_pane(socket: &Path, pane: &str, send: &muster_proto::SendToPane) -> Response {
    // The input connection says nothing about a pane it does not have, so a name nobody holds
    // is refused here, as the window refuses it, rather than reported sent.
    match snapshot(socket) {
        Ok(snapshot) if snapshot.panes.iter().any(|held| held.pane == pane) => {}
        Ok(_) => return not_held(socket, pane, "sent"),
        Err(trouble) => return failure(trouble.detail().to_string()),
    }
    let read = |rows| read_pane(socket, pane, Scope::Newest(rows)).map(|read| read.text);
    let before = if send.confirm { pane_text::before_sending(read) } else { String::new() };
    let typed = daemon::connect_welcomed(socket, ConnectionKind::Input).and_then(
        |(mut stream, welcome)| {
            let speaks = welcome.protocol.unwrap_or_default();
            if !send.keys.is_empty() && speaks.minor < daemon_proto::version::KEYS_IN_A_SEND {
                return Err(Trouble::Refused(format!(
                    "this machine's muster-daemon speaks protocol {speaks}, which cannot press \
                     keys, so nothing was sent. Update Muster; a new daemon takes over from an \
                     old one when the app starts."
                )));
            }
            let event = daemon_proto::InputEvent {
                pane: pane.to_string(),
                input: Some(daemon_proto::input_event::Input::Send(
                    daemon_proto::input_event::Send {
                        text: send.text.clone(),
                        enter: send.enter,
                        keys: send.keys.clone(),
                    },
                )),
            };
            connection::send(&mut stream, &event).map_err(|error| {
                Trouble::Unreachable(format!(
                    "nothing was sent to {pane}: the muster-daemon at {} would not take it \
                     ({error}).",
                    socket.display()
                ))
            })
        },
    );
    if let Err(trouble) = typed {
        return failure(trouble.detail().to_string());
    }
    if !send.confirm {
        return ok();
    }
    match pane_text::confirm(pane, &send.text, &before, read) {
        Ok(()) => ok(),
        Err(refusal) => failure(refusal),
    }
}

/// Asks the daemon to compact the agent in a pane, as the window asks it.
fn compact_pane(socket: &Path, pane: &str, focus: &str) -> Result<Response, Trouble> {
    let focus = Some(focus.trim().to_string()).filter(|focus| !focus.is_empty());
    let answer = asked(
        socket,
        pane_service(daemon_proto::pane_request::Request::Compact(
            daemon_proto::pane_request::Compact { pane: pane.to_string(), focus },
        )),
    )?;
    Ok(match answer.outcome() {
        daemon_proto::Outcome::NotThere => not_held(socket, pane, "compacted"),
        _ => changed(answer),
    })
}

/// Moves a pane into a new tab of its own, hidden or not, as a window's hide and show do. A pane
/// already where it was asked to be is left there.
fn move_hiding(socket: &Path, pane: &str, hidden: bool) -> Result<Response, Trouble> {
    let held = snapshot(socket)?;
    let Some(tab) = held.tabs.iter().find(|tab| {
        let mut panes = Vec::new();
        panes_of(tab.root.as_ref(), &mut panes);
        panes.iter().any(|held| held == pane)
    }) else {
        return Ok(not_held(socket, pane, if hidden { "hidden" } else { "shown" }));
    };
    if tab.hidden == hidden {
        return Ok(ok());
    }
    let tab = Minter::new(Mint::Drawn).tab().to_string();
    let placement = daemon_proto::Placement {
        r#where: Some(daemon_proto::placement::Where::NewTab(daemon_proto::placement::NewTab {
            tab,
            label: Some(daemon_proto::Label::default()),
            hidden,
        })),
    };
    let answer = asked_of_one_that_hides(
        socket,
        pane_service(daemon_proto::pane_request::Request::Move(daemon_proto::pane_request::Move {
            pane: pane.to_string(),
            placement: Some(placement),
        })),
        if hidden { "hide a pane" } else { "show a hidden pane" },
    )?;
    Ok(changed(answer))
}

/// Makes a pane quiet or lets it notify again: `quiet`, or the other of what it is now.
fn quiet_pane(socket: &Path, pane: &str, quiet: Option<bool>) -> Result<Response, Trouble> {
    let quiet = match quiet {
        Some(quiet) => quiet,
        None => !snapshot(socket)?.panes.iter().any(|held| held.pane == pane && held.quiet),
    };
    let answer = asked_of_one_that_hides(
        socket,
        pane_service(daemon_proto::pane_request::Request::Quiet(
            daemon_proto::pane_request::Quiet { pane: pane.to_string(), quiet },
        )),
        "make a pane quiet",
    )?;
    Ok(match answer.outcome() {
        daemon_proto::Outcome::NotThere => not_held(socket, pane, "changed"),
        _ => changed(answer),
    })
}

/// Calls a pane something, or with a blank name takes its name away, as a window's rename does.
fn rename_pane(socket: &Path, pane: &str, name: &str) -> Result<Response, Trouble> {
    let label = Some(name.trim().to_string()).filter(|name| !name.is_empty());
    let answer = asked(
        socket,
        pane_service(daemon_proto::pane_request::Request::Rename(
            daemon_proto::pane_request::Rename { pane: pane.to_string(), label },
        )),
    )?;
    Ok(match answer.outcome() {
        daemon_proto::Outcome::NotThere => not_held(socket, pane, "renamed"),
        _ => changed(answer),
    })
}

fn close_pane(socket: &Path, pane: &str) -> Result<Response, Trouble> {
    let answer = asked(
        socket,
        pane_service(daemon_proto::pane_request::Request::Close(
            daemon_proto::pane_request::Close { pane: pane.to_string() },
        )),
    )?;
    Ok(match answer.outcome() {
        daemon_proto::Outcome::NotThere => not_held(socket, pane, "closed"),
        _ => changed(answer),
    })
}

/// Makes a pane as a window's `pane new` does, by the rules that need nothing but this daemon:
/// beside the pane named, or with `--hidden` in a hidden tab of its own that starts where that
/// pane is. With no pane named there is no keyboard's pane to split, so it gets a tab of its own,
/// as a window showing nothing gives one; a tab no window holds joins the next that opens. The
/// keyboard `--focus` asks for is a window's, so with none it asks nothing.
///
/// The pane is told the caller's `$MUSTER_SOCKET`, as a window tells the panes it makes its own,
/// so that it drives the window once one is back.
fn make_pane(
    socket: &Path,
    split: &muster_proto::SplitPane,
    environment: &BTreeMap<String, String>,
) -> Result<Response, Trouble> {
    let said = |text: &str| (!text.is_empty()).then(|| text.to_string());
    let side = match split.side.as_str() {
        "left" => daemon_proto::Side::Left,
        "right" => daemon_proto::Side::Right,
        "up" => daemon_proto::Side::Up,
        "down" => daemon_proto::Side::Down,
        _ => {
            return Ok(failure(format!(
                "there is no side called {:?}, so nothing was made. They are left, right, up \
                 and down, and the CLI only sends those, so this is a bug in muster.",
                split.side
            )));
        }
    };
    let beside = said(&split.pane_id);
    let mut cwd = said(&split.cwd);
    let placement = match &beside {
        Some(pane) if !split.hidden => {
            daemon_proto::placement::Where::Beside(daemon_proto::placement::Beside {
                pane: pane.clone(),
                side: side.into(),
                ratio: (split.ratio > 0.0).then_some(split.ratio),
            })
        }
        _ => {
            if let Some(pane) = &beside {
                let held = snapshot(socket)?;
                let Some(held) = held.panes.iter().find(|held| &held.pane == pane) else {
                    return Ok(not_held(socket, pane, "made"));
                };
                cwd = cwd.or_else(|| said(&held.cwd));
            }
            daemon_proto::placement::Where::NewTab(daemon_proto::placement::NewTab {
                tab: Minter::new(Mint::Drawn).tab().to_string(),
                label: Some(daemon_proto::Label::default()),
                hidden: split.hidden,
            })
        }
    };
    let made = Minter::new(Mint::Drawn).pane().to_string();
    let window = environment
        .get(environment::WINDOW_SOCKET)
        .filter(|window| !window.is_empty())
        .map(|window| (environment::WINDOW_SOCKET.to_string(), window.clone()));
    let create = pane_service(daemon_proto::pane_request::Request::Create(
        daemon_proto::pane_request::Create {
            pane: made.clone(),
            placement: Some(daemon_proto::Placement { r#where: Some(placement) }),
            grid: None,
            cwd,
            env: window.into_iter().collect(),
            command: said(&split.run),
            label: said(&split.name),
            quiet: split.quiet,
        },
    ));
    let answer = match (split.hidden, split.quiet) {
        (true, _) => asked_of_one_that_hides(socket, create, "make a hidden pane")?,
        (false, true) => asked_of_one_that_hides(socket, create, "make a quiet pane")?,
        (false, false) => asked(socket, create)?,
    };
    Ok(match answer.outcome() {
        daemon_proto::Outcome::Done => Response {
            payload: Some(response::Payload::Made(muster_proto::Made { pane_id: made })),
        },
        daemon_proto::Outcome::NotThere => {
            not_held(socket, beside.as_deref().unwrap_or_default(), "made")
        }
        _ => failure(answer.reason),
    })
}

/// A daemon's answer to a change, as the window's own answer.
fn changed(answer: daemon_proto::Answer) -> Response {
    match answer.outcome() {
        daemon_proto::Outcome::Done | daemon_proto::Outcome::AlreadySo => ok(),
        _ => failure(answer.reason),
    }
}

// ---------------------------------------------------------------------------------------------
// What the daemon holds, as a window would describe it

/// The word a window paints for a pane, from the daemon's record alone.
///
/// The window's rule (`Pane::state` and `Pane::presented_state`, then `Attention::presented`):
/// an agent whose program has stopped reading is `stuck`, an idle agent that said what it is
/// waiting on is `waiting`, and a finish nobody has seen is `done` unless the agent is busy
/// again. A window also clears `done` for a pane it shows with the keyboard; with no window,
/// nobody has.
pub fn state_of(pane: &daemon_proto::Pane) -> AgentState {
    let waiting = pane.facts.as_ref().is_some_and(|facts| facts.waiting.is_some());
    let stuck = pane.agent.is_some() && pane.unread_input.is_some();
    let state = match pane.agent_state() {
        _ if stuck => AgentState::Stuck,
        daemon_proto::AgentState::Working => AgentState::Working,
        daemon_proto::AgentState::Blocked => AgentState::Blocked,
        daemon_proto::AgentState::Idle if waiting => AgentState::Waiting,
        daemon_proto::AgentState::Idle => AgentState::Idle,
        daemon_proto::AgentState::Unknown => AgentState::Unknown,
    };
    let busy = matches!(
        state,
        AgentState::Working | AgentState::Blocked | AgentState::Waiting | AgentState::Stuck
    );
    if !busy && pane.finished_unseen { AgentState::Done } else { state }
}

fn pane_state(daemon_id: &str, pane: &daemon_proto::Pane) -> muster_proto::PaneStateChanged {
    let facts = pane.facts.clone().unwrap_or_default();
    let said = facts != daemon_proto::AgentFacts::default();
    muster_proto::PaneStateChanged {
        daemon_id: daemon_id.to_string(),
        pane_id: pane.pane.clone(),
        state: state_of(pane).as_str().to_string(),
        // Read from the daemon itself, so it answered.
        last_state: String::new(),
        // The daemon does not say since when, and a window says when it first saw a pane.
        since_ms: 0,
        label: label_of(pane),
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
        adapter: harnesses::standing(pane.agent.as_deref(), adapter_of(pane)).as_str().to_string(),
        unread_input: pane.unread_input.as_ref().map(|unread| muster_proto::UnreadInput {
            bytes: unread.bytes,
            since_ms: i64::try_from(unread.since_ms).unwrap_or(i64::MAX),
            program: unread.program.clone(),
            pid: unread.pid,
        }),
    }
}

fn adapter_of(pane: &daemon_proto::Pane) -> Adapter {
    match pane.adapter() {
        daemon_proto::Adapter::Unsaid => Adapter::Unsaid,
        daemon_proto::Adapter::Reporting => Adapter::Reporting,
        daemon_proto::Adapter::Silent => Adapter::Silent,
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
    let (hidden, shown): (Vec<&daemon_proto::Tab>, Vec<&daemon_proto::Tab>) =
        snapshot.tabs.iter().partition(|tab| tab.hidden);
    let roster_tab = |tab: &daemon_proto::Tab| {
        let mut panes = Vec::new();
        panes_of(tab.root.as_ref(), &mut panes);
        muster_proto::RosterTab {
            daemon_ids: vec![machine.clone()],
            tab_id: tab.tab.clone(),
            label: tab.label.as_ref().and_then(|label| label.text.clone()).unwrap_or_default(),
            given_name: tab.label.as_ref().and_then(|label| label.text.clone()).unwrap_or_default(),
            panes: panes
                .into_iter()
                .map(|pane| {
                    let record = records.get(pane.as_str());
                    muster_proto::RosterPane {
                        daemon_id: machine.clone(),
                        label: record.map_or_else(String::new, |record| label_of(record)),
                        given_name: record
                            .and_then(|record| record.label.clone())
                            .unwrap_or_default(),
                        name: record
                            .and_then(|record| record.name())
                            .map(str::to_string)
                            .unwrap_or_default(),
                        subtitle: record.map_or_else(String::new, |record| record.title.clone()),
                        quiet: record.is_some_and(|record| record.quiet),
                        pane_id: pane,
                        ..muster_proto::RosterPane::default()
                    }
                })
                .collect(),
            ..muster_proto::RosterTab::default()
        }
    };
    let tabs = shown.into_iter().map(roster_tab).collect();
    let hidden = hidden.into_iter().map(roster_tab).collect();
    let directories: BTreeSet<String> =
        snapshot.panes.iter().map(|pane| pane.cwd.clone()).filter(|cwd| !cwd.is_empty()).collect();
    muster_proto::Window {
        roster: Some(muster_proto::RosterChanged {
            tabs,
            hidden,
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

/// Every tab's tree, every pane's place in it and every pane's size, for a layout asked of the
/// daemon with no window.
///
/// Each tab is one part the whole width, because with no window there is no second machine's
/// part beside it. The places are the window's own arithmetic over that one part
/// ([`places_in`]), so a frame here means what it means from a window. A daemon too old to say
/// its panes' sizes leaves them unknown.
fn lay_out(window: &mut muster_proto::Window, snapshot: &daemon_proto::Snapshot, socket: &Path) {
    let machine = window.daemons.first().map(|daemon| daemon.daemon_id.clone()).unwrap_or_default();
    window.layouts = snapshot
        .tabs
        .iter()
        .map(|tab| muster_proto::TabLayout {
            tab_id: tab.tab.clone(),
            regions: vec![muster_proto::ViewRegion {
                daemon_id: machine.clone(),
                tab_id: tab.tab.clone(),
                pane_id: tab.zoomed.clone().unwrap_or_default(),
                weight: 1.0,
                root: tab.root.as_ref().and_then(view_node),
                zoomed: tab.zoomed.is_some(),
                ..muster_proto::ViewRegion::default()
            }],
            places: tab
                .root
                .as_ref()
                .and_then(core_node)
                .map(|root| {
                    places_in(&root, Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 })
                        .into_iter()
                        .map(|(pane, rect)| muster_proto::PanePlace {
                            daemon_id: machine.clone(),
                            pane_id: pane.to_string(),
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: rect.height,
                            ..muster_proto::PanePlace::default()
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ..muster_proto::TabLayout::default()
        })
        .collect();
    window.grids = grids(socket)
        .into_iter()
        .map(|(pane, grid)| muster_proto::PaneGrid {
            daemon_id: machine.clone(),
            pane_id: pane,
            cols: grid.cols,
            rows: grid.rows,
        })
        .collect();
}

/// A daemon's tree as the core places one.
fn core_node(node: &daemon_proto::Node) -> Option<ViewNode> {
    Some(match node.node.as_ref()? {
        daemon_proto::node::Node::Pane(pane) => ViewNode::Pane(ViewPane {
            id: PaneId::new(pane),
            link_socket_path: None,
            font_size_offset: 0,
            bridge_restarts: 0,
        }),
        daemon_proto::node::Node::Split(split) => ViewNode::Split {
            axis: match split.axis() {
                daemon_proto::Axis::Rows => SplitAxis::Rows,
                daemon_proto::Axis::Columns | daemon_proto::Axis::Unspecified => SplitAxis::Columns,
            },
            ratio: split.ratio,
            first: Box::new(core_node(split.first.as_deref()?)?),
            second: Box::new(core_node(split.second.as_deref()?)?),
        },
    })
}

/// A daemon's tree in a window's vocabulary.
fn view_node(node: &daemon_proto::Node) -> Option<muster_proto::ViewNode> {
    let node = match node.node.as_ref()? {
        daemon_proto::node::Node::Pane(pane) => {
            muster_proto::view_node::Node::Pane(muster_proto::ViewPane {
                pane_id: pane.clone(),
                ..muster_proto::ViewPane::default()
            })
        }
        daemon_proto::node::Node::Split(split) => {
            muster_proto::view_node::Node::Split(Box::new(muster_proto::ViewSplit {
                axis: match split.axis() {
                    daemon_proto::Axis::Rows => "rows",
                    daemon_proto::Axis::Columns | daemon_proto::Axis::Unspecified => "columns",
                }
                .to_string(),
                ratio: split.ratio,
                first: split.first.as_deref().and_then(view_node).map(Box::new),
                second: split.second.as_deref().and_then(view_node).map(Box::new),
            }))
        }
    };
    Some(muster_proto::ViewNode { node: Some(node) })
}

/// Every pane's size, or none from a daemon that cannot say.
fn grids(socket: &Path) -> BTreeMap<String, daemon_proto::Grid> {
    let asked = asked(
        socket,
        daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(daemon_proto::session_request::Request::ReadGrids(
                daemon_proto::session_request::ReadGrids {},
            )),
        }),
    );
    match asked.ok().and_then(|answer| answer.detail) {
        Some(daemon_proto::answer::Detail::Grids(grids)) => grids.panes.into_iter().collect(),
        _ => BTreeMap::new(),
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
/// What to call a pane in a row: the name somebody gave it, or its word and its directory, as a
/// window's row reads (`muster_core::roster::pane_label`) less the harness, which only a window's
/// detection names here.
fn label_of(pane: &daemon_proto::Pane) -> String {
    let given = pane.label.as_deref().map(str::trim).filter(|label| !label.is_empty());
    match (given, pane.word.as_deref().filter(|word| !word.is_empty())) {
        (Some(given), _) => given.to_string(),
        (None, Some(word)) => match directory_name(&pane.cwd) {
            directory if directory.is_empty() => word.to_string(),
            directory => format!("{word} · {directory}"),
        },
        (None, None) => directory_name(&pane.cwd),
    }
}

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
    until: Until,
    /// Whether a tab's arrangement changing is news, for a layout being drawn again.
    layout: bool,
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
    let until = Until::parse(&watch.until, watch.context_at_least).map_err(Trouble::Refused)?;
    let socket = daemon::socket_or_refusal(environment)?;
    let mut stream = daemon::connect(&socket, ConnectionKind::Control)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let subscribe = daemon_proto::Request {
        id: 1,
        service: Some(daemon_proto::request::Service::Session(daemon_proto::SessionRequest {
            request: Some(daemon_proto::session_request::Request::Subscribe(
                daemon_proto::session_request::Subscribe { attends: false },
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
    let mut pane_ids = watch.pane_ids.clone();
    addressed(&mut pane_ids.iter_mut().collect::<Vec<_>>(), &snapshot)?;
    let panes = if pane_ids.is_empty() {
        None
    } else {
        for named in &pane_ids {
            if !snapshot.panes.iter().any(|pane| &pane.pane == named) {
                return Err(Trouble::Refused(format!(
                    "the muster-daemon at {} holds no pane called {named}, so there is nothing \
                     to watch. Either it closed, or the name is from another machine - `muster \
                     window` lists the panes this one has.",
                    socket.display()
                )));
            }
        }
        Some(pane_ids.into_iter().collect())
    };
    let mut watching = Watching {
        stream,
        socket,
        machine: this_machine(),
        panes,
        until,
        layout: watch.layout,
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
        if !self.until.is_wait() {
            for pane in watched {
                self.ready.push_back(self.said(pane));
            }
            return;
        }
        let arrived: Vec<&daemon_proto::Pane> =
            watched.iter().copied().filter(|pane| self.arrived(pane)).collect();
        if !arrived.is_empty() {
            for pane in arrived {
                self.ready.push_back(self.said(pane));
            }
            self.last(Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) });
            return;
        }
        if let Some(stranded) = watched.into_iter().find_map(|pane| self.stranded(pane)) {
            self.last(stranded);
        }
    }

    /// The refusal ending a wait on a pane that went stuck, on the window's terms
    /// (`muster_seam::watch`): only a pane the wait names.
    fn stranded(&self, pane: &daemon_proto::Pane) -> Option<Response> {
        let named = self.panes.as_ref().is_some_and(|panes| panes.contains(&pane.pane));
        if !named || !self.until.stranded(state_of(pane)) {
            return None;
        }
        let program =
            pane.unread_input.as_ref().map(|unread| (unread.program.as_str(), unread.bytes));
        Some(failure(self.until.stranded_because(&pane.pane, program)))
    }

    fn said(&self, pane: &daemon_proto::Pane) -> Response {
        let mut state = pane_state(&self.machine, pane);
        // As from a window: a wait prints the pane that got there and nothing else.
        if self.until.is_wait() {
            state.label.clear();
        }
        Response { payload: Some(response::Payload::PaneState(state)) }
    }

    fn last(&mut self, response: Response) {
        self.ready.push_back(response);
        self.ended = true;
    }

    fn watches(&self, pane: &str) -> bool {
        self.panes.as_ref().is_none_or(|panes| panes.contains(pane))
    }

    fn arrived(&self, pane: &daemon_proto::Pane) -> bool {
        let context_used = pane.facts.as_ref().and_then(|facts| facts.context_used);
        self.until.met(state_of(pane), context_used)
    }

    fn changed(&mut self, pane: &daemon_proto::Pane) {
        if self.ended || !self.watches(&pane.pane) {
            return;
        }
        let state = state_of(pane);
        let again = self.sent.insert(pane.pane.clone(), state) == Some(state);
        if !self.until.is_wait() {
            if !again {
                self.ready.push_back(self.said(pane));
            }
            return;
        }
        // Not deduplicated: a wait on context is met by a report that changes the agent's facts
        // and leaves its state where it was.
        if self.arrived(pane) {
            self.ready.push_back(self.said(pane));
            self.last(Response { payload: Some(response::Payload::Ok(muster_proto::Ok {})) });
        } else if let Some(stranded) = self.stranded(pane) {
            self.last(stranded);
        }
    }

    fn closed(&mut self, pane: &str) {
        if self.ended {
            return;
        }
        let named = self.panes.as_ref().is_some_and(|panes| panes.contains(pane));
        let was_sent = self.sent.remove(pane).is_some();
        if !self.until.is_wait() {
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
            self.last(failure(format!(
                "pane {pane} closed before it was {}, so there is nothing left to wait for. \
                 Whatever was running in it ended, or somebody closed it; `muster window` lists \
                 what this machine still holds.",
                self.until.spelled()
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
                    Some(
                        daemon_proto::event::Event::TabOpened(_)
                        | daemon_proto::event::Event::TabChanged(_)
                        | daemon_proto::event::Event::TabClosed(_),
                    ) if self.layout => {
                        self.ready.push_back(Response {
                            payload: Some(response::Payload::LayoutMoved(
                                muster_proto::LayoutMoved {},
                            )),
                        });
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

    /// A pane is made with no window only on this machine: a window knows which machine its
    /// `--daemon` names, and nothing here does.
    #[test]
    fn a_pane_is_made_without_a_window_only_on_this_machine() {
        let asking = |split: muster_proto::SplitPane| {
            can_answer(&Request::new(request::Payload::SplitPane(split)))
        };
        assert!(asking(muster_proto::SplitPane { pane_id: "p1".into(), ..Default::default() }));
        assert!(!asking(muster_proto::SplitPane {
            pane_id: "p1".into(),
            new_pane_daemon_id: "devenv".into(),
            ..Default::default()
        }));
        assert!(!asking(muster_proto::SplitPane {
            daemon_id: "devenv".into(),
            ..Default::default()
        }));
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
