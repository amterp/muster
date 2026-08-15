//! What each request means.
//!
//! Bytes in, bytes out, no FFI: this is where the seam's behavior can be tested without a
//! shell, a window or a linker. [`crate::ffi`] is the shim that lets a C caller reach it.

use std::collections::BTreeMap;

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::diagnostics::sink::JsonLinesSink;
use muster_core::fields;

use muster_core::composition::{DaemonId, RegionId, Step};
use muster_core::config;
use muster_core::input::{CompositionOutcome, ScrollDirection, composition_outcome};
use muster_core::intent::{BackendIntent, Branch, Side};
use muster_core::mirror::backend::{PaneId, TabId};

use crate::convert;
use crate::proto::{self, Request, Response, request, response};
use crate::session::{self, AttachError, AttachedPane};
use prost::Message;

/// Answers one encoded request.
///
/// Total by construction. A request that will not decode is still answered, because the
/// alternative is a shell that cannot tell "the core refused" from "the core is gone", and
/// those want very different reactions.
pub fn dispatch(request: &[u8]) -> Vec<u8> {
    let response = match Request::decode(request) {
        Ok(request) => handle(request),
        Err(error) => Response::failure(format!(
            "the core could not decode a request ({error}). Whatever the shell was asking \
             for did not happen and will not be retried. The two sides are generated from \
             one proto/muster.proto in one build, so this means a stale libmuster.dylib \
             rather than a schema disagreement - check that ./dev built the core this \
             shell is linked against."
        )),
    };
    response.encode_to_vec()
}

fn handle(request: Request) -> Response {
    let Some(payload) = request.payload else {
        return Response::failure(
            "the core was handed a request with no payload, so there was nothing to do. \
             This is a bug in the shell's request building rather than a state worth \
             recovering from; the field is a oneof and every arm sets it.",
        );
    };

    match payload {
        request::Payload::Startup(startup) => start(&startup),
        request::Payload::LogRecord(record) => write(record),
        request::Payload::AttachPane(attach) => attach_pane(&attach.pane_id),
        request::Payload::OpenWindow(_) => open_window(),
        request::Payload::CreateTab(create) => create_tab(&create),
        request::Payload::BridgeExited(exited) => bridge_exited(&exited),
        request::Payload::KeyDown(down) => with_pane("a keystroke", |pane| key_down(pane, &down)),
        request::Payload::KeyUp(up) => {
            with_pane("a key release", |pane| send_key(pane, up.key.as_ref()))
        }
        request::Payload::SendText(text) => with_pane("text", |pane| {
            pane.input.send_text(&text.text);
            Response::ok()
        }),
        request::Payload::Paste(paste) => with_pane("a paste", |pane| {
            pane.input.paste(&paste.text);
            Response::ok()
        }),
        request::Payload::SplitPane(split) => match convert::axis(&split.axis) {
            Some(axis) => act(&split.daemon_id, &split.pane_id, |pane| BackendIntent::SplitPane {
                pane,
                axis,
                // Zero is proto3's unset, and a divider at the very edge is not a thing
                // anyone asks for, so the two are safely the same answer here.
                ratio: (split.ratio > 0.0).then_some(split.ratio),
                // Empty is the daemon's own rule rather than this process's directory, and
                // for a split that rule is "wherever the pane you split was".
                cwd: (!split.cwd.is_empty()).then(|| split.cwd.clone()),
            }),
            None => Response::failure(format!(
                "the core does not know a split axis called {:?}, so nothing was split. \
                 Only columns and rows exist; the shell builds this from a fixed set, so \
                 this is a bug there.",
                split.axis
            )),
        },
        request::Payload::ClosePane(close) => {
            act(&close.daemon_id, &close.pane_id, |pane| BackendIntent::ClosePane { pane })
        }
        request::Payload::ResizePane(resize) => resize_pane(&resize),
        request::Payload::ZoomPane(zoom) => {
            act(&zoom.daemon_id, &zoom.pane_id, |pane| BackendIntent::ZoomPane { pane })
        }
        request::Payload::FocusPane(focus) if focus.pane_id.is_empty() => Response::failure(
            "a focus request named no pane, so the keyboard stayed where it was. Unlike every \
             other pane request, an empty id has no useful meaning here - it would ask to \
             focus whatever is already focused - so the shell building this has a bug.",
        ),
        request::Payload::FocusPane(focus) => match resolve_daemon(&focus.daemon_id) {
            Ok(daemon) => answer(session::focus(&daemon, &PaneId::new(focus.pane_id))),
            Err(refusal) => refusal,
        },
        request::Payload::WindowFocus(focus) => {
            session::window_focused(focus.focused);
            Response::ok()
        }
        request::Payload::SetRegionBoundary(set) => move_region_boundary(&set),
        request::Payload::FocusRelative(step) => match Step::parse(&step.direction) {
            Some(direction) => answer(session::step(direction)),
            None => Response::failure(format!(
                "the core does not know a step called {:?}, so the keyboard stayed where it \
                 was. Only next, previous, left, right, up and down exist; the shell builds \
                 this from a fixed set, so this is a bug there.",
                step.direction
            )),
        },
        request::Payload::SetSplitRatio(set) => match resolve_daemon(&set.daemon_id) {
            Ok(daemon) => submit(
                &daemon,
                &BackendIntent::SetSplitRatio {
                    tab: TabId::new(set.tab_id),
                    path: set
                        .path
                        .into_iter()
                        .map(|second| if second { Branch::Second } else { Branch::First })
                        .collect(),
                    ratio: set.ratio,
                },
            ),
            Err(refusal) => refusal,
        },
        request::Payload::Scroll(scroll) => {
            with_pane("a scroll", |pane| match ScrollDirection::parse(&scroll.direction) {
                Some(direction) => {
                    pane.input.scroll(direction, scroll.lines.try_into().unwrap_or(u16::MAX));
                    Response::ok()
                }
                None => Response::failure(format!(
                    "the core does not know a scroll direction called {:?}, so the wheel did \
                     nothing. Only up and down exist; the shell builds this from a fixed set, \
                     so this is a bug there.",
                    scroll.direction
                )),
            })
        }
    }
}

/// One press, after the input method has had its turn.
///
/// The arbitration happens here rather than in the shell because it is a decision, and
/// exactly one thing may come of a press: the text a composition produced, or the key
/// itself, or nothing at all.
fn key_down(pane: &AttachedPane, down: &proto::KeyDown) -> Response {
    match composition_outcome(down.was_composing, down.committed.as_deref(), down.still_composing) {
        CompositionOutcome::SendNothing => Response::ok(),
        CompositionOutcome::SendText(text) => {
            pane.input.send_text(&text);
            Response::ok()
        }
        CompositionOutcome::SendKey => send_key(pane, down.key.as_ref()),
    }
}

fn send_key(pane: &AttachedPane, key: Option<&proto::KeyEvent>) -> Response {
    let Some(key) = key else {
        return Response::failure(
            "the core was handed a keystroke with no key in it, so nothing reached the pane. \
             This is a bug in the shell's request building rather than a state worth \
             recovering from.",
        );
    };
    match convert::key(key) {
        Ok(key) => {
            pane.input.send(&key);
            Response::ok()
        }
        Err(reason) => Response::failure(reason),
    }
}

/// Runs something against the pane the keyboard feeds, or explains why there is not one.
///
/// A bare `muster` legitimately has no pane - it is the renderer check - so this is a
/// refusal rather than an error, and it says which input went nowhere so a log reads as
/// something other than silence.
///
/// The session's lock is released before `act` runs. Sending can be a round trip to a
/// daemon, and holding the session across one would stall every event arriving from every
/// other daemon behind a wedged one.
fn with_pane(what: &str, act: impl FnOnce(&AttachedPane) -> Response) -> Response {
    match session::keyboard_pane() {
        Some(pane) => act(&pane),
        None => Response::failure(format!(
            "no pane has this window's keyboard, so {what} went nowhere. A window with no \
             pane named is the renderer check and this is expected there; anywhere else it \
             means the attach failed earlier, or the pane it succeeded on has since exited, \
             and that is the event worth reading."
        )),
    }
}

/// Builds an intent about a pane and asks for it.
///
/// An empty pane id means the one this window's keyboard feeds, and an empty daemon means
/// the daemon that pane is on, because that is what a keybinding means and a keybinding is
/// the common caller. A click sends both, having read them off the view it was rendered
/// from; a CLI that names a pane gets the pane it named.
fn act(daemon_id: &str, pane_id: &str, build: impl FnOnce(PaneId) -> BackendIntent) -> Response {
    let daemon = match resolve_daemon(daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return refusal,
    };
    let pane = if pane_id.is_empty() {
        match session::focused_pane() {
            Some(pane) => pane,
            None => {
                return Response::failure(
                    "no pane has this window's keyboard, so there was nothing to act on. A \
                     request that names no pane means the focused one, and this window has \
                     none - the attach failed earlier, or the pane it succeeded on exited.",
                );
            }
        }
    } else {
        PaneId::new(pane_id)
    };
    submit(&daemon, &build(pane))
}

/// Makes a tab beside a pane, in the directory that pane is in.
///
/// The directory is resolved here rather than left to the daemon. A new tab has nothing to
/// inherit from, so herdr would start it in a home directory - and the answer somebody
/// pressing the key means is "where I already am", which the mirror already knows.
///
/// Which workspace, too, and that one is not a nicety: `tab.create` takes a workspace and
/// ignores keys it does not know, so a request that named the pane instead would be accepted
/// and put the tab wherever that daemon last had focus.
fn create_tab(create: &proto::CreateTab) -> Response {
    let daemon = match resolve_daemon(&create.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return refusal,
    };
    let pane = if create.pane_id.is_empty() {
        match session::focused_pane() {
            Some(pane) => pane,
            None => {
                return Response::failure(
                    "no pane has this window's keyboard, so there is no workspace to make a \
                     tab in. A request that names no pane means the focused one, and this \
                     window has none.",
                );
            }
        }
    } else {
        PaneId::new(&create.pane_id)
    };

    let Some((workspace, inherited)) = session::workspace_of(&daemon, &pane) else {
        return Response::failure(format!(
            "the daemon {daemon} holds no pane called {pane}, so there is no workspace to put \
             a tab in and nothing was made. Most likely it closed while this was in flight."
        ));
    };
    let cwd = if create.cwd.is_empty() { inherited } else { Some(create.cwd.clone()) };
    submit(&daemon, &BackendIntent::CreateTab { workspace, cwd })
}

/// The daemon a request means, given what it named.
///
/// Empty is the ordinary case rather than an omission: every menu item sends it, because a
/// menu item is about whatever is in front of the user. Only a window with nothing attached
/// has no answer, and that is the renderer check.
fn resolve_daemon(daemon_id: &str) -> Result<DaemonId, Response> {
    if !daemon_id.is_empty() {
        return Ok(DaemonId::new(daemon_id));
    }
    session::focused_daemon().ok_or_else(|| {
        Response::failure(
            "this window has no daemon its keyboard is on, so a request that named none had \
             nothing to act on. A window with nothing attached looks like this, and so does \
             one whose every region closed.",
        )
    })
}

/// Moves the line between two regions of the window.
///
/// The one arrangement no daemon is asked about, so unlike every other drag this answers
/// `ok` for a change that has already happened rather than for a request somebody may refuse.
fn move_region_boundary(set: &proto::SetRegionBoundary) -> Response {
    let Some(region) = region_id(&set.region_id) else {
        return Response::failure(format!(
            "the core does not know a region called {:?}, so no line moved. A region is named \
             `r0`, `r1` and so on, and the shell reads these off the view it rendered - so \
             this is a bug there.",
            set.region_id
        ));
    };
    session::set_region_boundary(region, set.ratio);
    Response::ok()
}

/// A region, read back from the way the view spells one.
///
/// `r0`, `r1`, and so on - the same rendering the view publishes, parsed here rather than
/// the seam carrying a number the shell would have to strip the prefix off. Anything else is
/// a refusal, because a region id the core cannot read is a shell that built one itself.
fn region_id(named: &str) -> Option<RegionId> {
    named.strip_prefix('r')?.parse().ok().map(RegionId::new)
}

fn submit(daemon: &DaemonId, intent: &BackendIntent) -> Response {
    answer(session::submit(daemon, intent))
}

fn answer(outcome: Result<(), String>) -> Response {
    match outcome {
        Ok(()) => Response::ok(),
        Err(refusal) => Response::failure(format!(
            "the daemon did not make that change: {refusal} Nothing about the session moved, \
             and the window still shows what it showed before - which is the honest answer \
             rather than a view that pretends."
        )),
    }
}

/// Opens the window onto whatever the daemons hold, which is what a bare `muster` asks for.
fn open_window() -> Response {
    match session::open() {
        Ok(()) => Response::ok(),
        Err(detail) => Response::failure(format!(
            "{detail} This window has no session behind it, so it renders nothing and \
             ignores the keyboard."
        )),
    }
}

/// Grows a pane against its neighbour, by a step.
fn resize_pane(resize: &proto::ResizePane) -> Response {
    let Some(direction) = Side::parse(&resize.direction) else {
        return Response::failure(format!(
            "the core does not know a direction called {:?}, so nothing was resized. They are \
             left, right, up and down.",
            resize.direction
        ));
    };
    act(&resize.daemon_id, &resize.pane_id, |pane| BackendIntent::ResizePane {
        pane,
        direction,
        // Zero is proto3's unset, and resizing a pane by nothing is not a thing anyone asks
        // for, so the two are safely the same answer.
        amount: (resize.amount > 0.0).then_some(resize.amount),
    })
}

/// The shell saying nothing is painting one of its panes any more.
///
/// Answered by asking that daemon what it holds, because the commonest reason a bridge ends
/// is that the pane it was painting is gone - and herdr can drop a pane without an event, so
/// this is sometimes the only notice there is. A pane the daemon still holds survives the
/// re-read unchanged and keeps its dead surface, which is the honest outcome: the shell can
/// say the bridge died, and it cannot say the pane did.
///
/// Always `Ok`. Nothing was asked for, so there is nothing to refuse.
fn bridge_exited(exited: &proto::BridgeExited) -> Response {
    session::bridge_exited(&exited.daemon_id, &exited.pane_id, exited.process_alive);
    Response::ok()
}

fn attach_pane(pane_id: &str) -> Response {
    match session::attach(pane_id) {
        Ok(pane) => Response {
            payload: Some(response::Payload::Attached(proto::Attached {
                control_socket_path: pane.control_socket_path.clone(),
            })),
        },
        Err(AttachError::Unreachable(detail)) => Response::failure(format!(
            "{detail} Nothing is known about this pane, so the window will render nothing."
        )),
        Err(AttachError::NoSuchPane { pane, held, dropped }) => Response::failure(format!(
            "no daemon holds a pane called {pane}, so this window has nothing to show and \
             would render and ignore the keyboard. The daemon answered with {held} pane(s); \
             `herdr pane list` names them. {dropped} entries in that answer did not read, \
             so if this id is one of them the problem is a herdr whose payload has moved \
             rather than a pane that is missing."
        )),
        Err(AttachError::NoChannel(detail)) => Response::failure(format!(
            "the core could not open a channel to this pane: {detail} Nothing typed into it \
             would reach it, so it would render and ignore the keyboard."
        )),
    }
}

/// Turns logging on and attaches whatever the config file names.
///
/// An empty log path is not a failure: it is what a release build asks for, and what the
/// shell sends when the user has not opted in. Logging is set up first so that the config
/// file's own account of itself has somewhere to go.
fn start(startup: &proto::Startup) -> Response {
    // Before the config, because applying one attaches the daemons it names and attaching a
    // local one may have to start it.
    session::set_daemon_binary(&startup.herdr_path);
    // Before the config too, and for a sharper reason: applying a config attaches daemons,
    // attaching publishes, and a publish before this is one that would write the arrangement
    // out to nowhere - or worse, read it back after it had been replaced.
    session::set_state_path(&startup.state_path);

    if startup.log_path.is_empty() {
        apply_config(&startup.config_path);
        return Response::ok();
    }
    let Some(sink) = JsonLinesSink::open(&startup.log_path) else {
        return Response::failure(format!(
            "the core could not open {} for logging, so this run leaves no record. \
             Everything else works; a bug report from it will just be missing the timeline \
             that usually explains what happened. Check that the directory exists and is \
             writable.",
            startup.log_path
        ));
    };
    let level = if startup.log_level.is_empty() {
        LogLevel::Debug
    } else {
        match LogLevel::parse(&startup.log_level) {
            Some(level) => level,
            None => {
                return Response::failure(format!(
                    "the core does not know a log level called {:?}, so logging stayed off \
                     and this run leaves no record. Valid levels are trace, debug, info, \
                     warn and error; check MUSTER_LOG_LEVEL.",
                    startup.log_level
                ));
            }
        }
    };
    let process =
        if startup.process.is_empty() { "app".to_string() } else { startup.process.clone() };
    log::install(Box::new(sink), process, level);

    // First record of the run, and the one a bug report is read against: which engine will
    // encode this session's keystrokes. libghostty-vt is reproduced from deps/ghostty.pin
    // rather than installed, so "the pin in the repo today" is not an answer about a run
    // from last week.
    log::info(
        "core.start",
        fields! {
            "vt_engine" => muster_vt::engine_version().unwrap_or_else(|| "unknown".to_string()),
            // The other half of the same question: which daemon this run would start. A
            // window with no session behind it is nearly always one of these two being
            // absent, and both belong in the first record rather than in the failure.
            "daemon_binary" => if startup.herdr_path.is_empty() {
                "(none staged)".to_string()
            } else {
                startup.herdr_path.clone()
            },
        },
    );
    apply_config(&startup.config_path);
    Response::ok()
}

/// Reads the config file and starts following the daemons it names.
///
/// Reading is here rather than in the core because it is I/O, and the core's rule is that it
/// arrives through an edge. What comes back is a pure parse of the text, judged by the corpus.
///
/// Nothing here is fatal, and the response says nothing about it. A config that could not be
/// read leaves Muster doing what a Muster with no config does - find the daemon on this
/// machine - which is a working window, and the alternative is refusing to open one because a
/// file has a typo. What it must never be is silent, so each way of failing writes the line
/// that explains the window somebody is about to look at.
fn apply_config(path: &str) {
    if path.is_empty() {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            log::warn(
                "config.unreadable",
                fields! {
                    "path" => path.to_string(),
                    "detail" => error.to_string(),
                    "impact" => "no daemon named in that file is attached, so the window shows                                  only the daemon on this machine",
                    "check" => "whether the path exists and is readable; the shell only sends                                 one it has already seen",
                },
            );
            return;
        }
    };
    match config::parse(&text) {
        Ok(config) => {
            log::info(
                "config.read",
                fields! { "path" => path.to_string(), "daemons" => config.daemons.len().to_string() },
            );
            session::follow_configured(&config);
        }
        Err(refusal) => log::warn(
            "config.refused",
            fields! {
                "path" => path.to_string(),
                "detail" => refusal,
                "impact" => "no daemon named in that file is attached, so the window shows                              only the daemon on this machine",
            },
        ),
    }
}

fn write(record: proto::LogRecord) -> Response {
    let Some(level) = LogLevel::parse(&record.level) else {
        return Response::failure(format!(
            "the core does not know a log level called {:?}, so the {:?} record was \
             dropped. Whatever it was reporting is now invisible. The shell builds this \
             field from a fixed set, so this is a bug there rather than a configuration \
             problem.",
            record.level, record.event
        ));
    };
    // The map arrives unordered, as protobuf maps do, and records are sorted so that two
    // runs of the same code produce the same bytes.
    log::emit(level, &record.event, record.fields.into_iter().collect::<BTreeMap<_, _>>());
    Response::ok()
}
