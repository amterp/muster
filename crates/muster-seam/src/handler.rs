//! What each request means.
//!
//! Bytes in, bytes out, no FFI: this is where the seam's behavior can be tested without a
//! shell, a window or a linker. [`crate::ffi`] is the shim that lets a C caller reach it.

use std::collections::BTreeMap;

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::diagnostics::sink::JsonLinesSink;
use muster_core::fields;

use muster_core::composition::{DaemonId, FontSizeChange, Frame, RegionId, Step};
use muster_core::config::{self, CursorStyle};
use muster_core::find::Needle;
use muster_core::font::{self, FontReport};
use muster_core::input::{CompositionOutcome, Modifiers, ScrollDirection, composition_outcome};
use muster_core::intent::{BackendIntent, Branch, Side};
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::problems::Severity;
use muster_core::roster::TabStep;

use crate::proto::{self, Request, Response, event, request, response};
use crate::session::{self, AttachError, AttachedPane, Keyboard};
use crate::{command, convert};
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

    // A `tab_then_pane` chord is armed by one request and spent by the next, so the rule that
    // ends it is stated here rather than at every caller that could: **a request that only
    // reads keeps it, and anything that changes something clears it.** So the second press
    // uses it, and a keystroke into a pane, an Escape, another action, a click or a divider
    // drag all take it away. One rule a person can hold, in the one place every request
    // passes through - a list of callers would be a list somebody eventually forgets to add
    // to, and the forgotten one is a chord that fires two gestures later.
    //
    // A request added after this defaults to clearing, which is the safe direction: an
    // over-eager disarm costs a chord, and a stuck one is a window whose numbers lie.
    //
    // It costs one uncontended lock on every request, including each keystroke, and that is
    // affordable next to the protobuf decode two lines above it - which allocates, on the same
    // path, for every one of them. Skipping it when nothing can be armed would mean reading
    // whether anything is, which is the same lock.
    if !only_reads(&payload) {
        session::disarm();
    }

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
        request::Payload::SplitPane(split) => split_pane(&split),
        request::Payload::ClosePane(close) => {
            act(&close.daemon_id, &close.pane_id, Keyboard::Follows, |pane| {
                BackendIntent::ClosePane { pane }
            })
        }
        request::Payload::ReadBindings(_) => read_bindings(),
        request::Payload::ReadWindow(_) => read_window(),
        request::Payload::SendToPane(send) => send_to_pane(&send),
        request::Payload::ReadAppearance(_) => read_appearance(),
        request::Payload::ReadWindowFrame(read) => read_window_frame(&read.screens),
        request::Payload::ReportFontFamily(report) => report_font_family(&report),
        request::Payload::SetWindowFrame(set) => {
            let frame = set.frame.unwrap_or_default();
            session::set_window_frame(frame.rect.map(read_rect), frame.full_screen);
            Response::ok()
        }
        request::Payload::ResizePane(resize) => resize_pane(&resize),
        request::Payload::ToggleSidebar(_) => {
            session::toggle_sidebar();
            Response::ok()
        }
        request::Payload::AdjustFontSize(adjust) => adjust_font_size(&adjust.change),
        request::Payload::ReloadConfig(_) => reload_config(),
        request::Payload::ZoomPane(zoom) => {
            act(&zoom.daemon_id, &zoom.pane_id, Keyboard::Follows, |pane| BackendIntent::ZoomPane {
                pane,
            })
        }
        request::Payload::FocusPane(focus) if focus.pane_id.is_empty() => Response::failure(
            "a focus request named no pane, so the keyboard stayed where it was. Unlike every \
             other pane request, an empty id has no useful meaning here - it would ask to \
             focus whatever is already focused - so the shell building this has a bug.",
        ),
        request::Payload::FocusPane(focus) => match resolve_daemon(&focus.daemon_id) {
            Ok(daemon) => answer(session::focus(&daemon, &PaneId::new(focus.pane_id))),
            Err(refusal) => *refusal,
        },
        request::Payload::WindowFocus(focus) => {
            session::window_focused(focus.focused);
            Response::ok()
        }
        request::Payload::SetRegionBoundary(set) => move_region_boundary(&set),
        request::Payload::FocusRelative(step) => focus_relative(&step.direction),
        request::Payload::FocusTabRelative(step) => step_tab(&step.direction),
        request::Payload::FocusPaneAt(at) => focus_pane_at(at.place),
        request::Payload::FocusTab(tab) => focus_tab(&tab.daemon_id, &tab.tab_id),
        request::Payload::ArrangePane(arrange) => arrange_pane(&arrange),
        request::Payload::SetSplitRatio(set) => set_split_ratio(set),
        request::Payload::Scroll(scroll) => scroll_pane(&scroll),
        request::Payload::RenamePane(rename) => rename_pane(&rename),
        request::Payload::RenameTab(rename) => rename_tab(&rename),
        request::Payload::Find(find) => find_in_pane(&find),
        request::Payload::FindStep(step) => step_find(&step.direction),
        request::Payload::EndFind(_) => {
            session::end_find();
            Response::ok()
        }
        // Answered when the panes have been handed back, not when the message was read. The
        // shell is holding its own termination open on this reply, which is the whole point:
        // everything here has to happen while its bridges are still alive to relay it.
        request::Payload::Quitting(_) => {
            session::quitting();
            Response::ok()
        }
        // The rule at the top has already done this, since this is not a read. Said again
        // here because an arm reading `Response::ok()` alone would look like a request that
        // does nothing, and the second call is free: `disarm` takes the arm and finds none.
        request::Payload::EndNumberedChord(_) => {
            session::disarm();
            Response::ok()
        }
    }
}

/// Whether a request only asks a question, and so leaves an armed numbered chord alone.
///
/// Deliberately a short allowlist with everything else falling through: see the rule at the
/// top of [`handle`]. Two of these matter more than they look. The shell logs through the core,
/// often, and a run log that disarmed the chords would make the prototype work only in a build
/// nobody was watching. And `FocusPaneAt` is the second press itself - the one request whose
/// whole job is to spend what the first one armed.
fn only_reads(payload: &request::Payload) -> bool {
    matches!(
        payload,
        request::Payload::LogRecord(_)
            | request::Payload::ReadBindings(_)
            | request::Payload::ReadWindow(_)
            | request::Payload::ReadAppearance(_)
            | request::Payload::ReadWindowFrame(_)
            | request::Payload::ReportFontFamily(_)
            | request::Payload::FocusPaneAt(_)
    )
}

/// Looks for something in a pane, and puts the first match on screen.
///
/// Named like every other pane request: an empty pane means the one the keyboard is on,
/// which is the pane a find bar is drawn over and so the only case a chord produces.
fn find_in_pane(find: &proto::Find) -> Response {
    let daemon = match resolve_daemon(&find.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return *refusal,
    };
    let pane = if find.pane_id.is_empty() {
        match session::focused_pane() {
            Some(pane) => pane,
            None => {
                return Response::failure(
                    "no pane has this window's keyboard, so there was nothing to search. A find \
                     that names no pane means the focused one, and this window has none - the \
                     attach failed earlier, or the pane it succeeded on exited.",
                );
            }
        }
    } else {
        PaneId::new(&find.pane_id)
    };
    found(session::find(&daemon, &pane, &Needle::new(&find.needle)))
}

/// Walks the matches of the search already open.
fn step_find(direction: &str) -> Response {
    let forward = match direction {
        "next" => true,
        "previous" => false,
        _ => {
            return Response::failure(format!(
                "the core does not know a find step called {direction:?}, so the selected match \
                 stayed where it was. Only next and previous exist; the shell builds this from a \
                 fixed set, so this is a bug there."
            ));
        }
    };
    found(session::step_find(forward))
}

fn found(answer: Result<session::Findings, String>) -> Response {
    match answer {
        Ok(findings) => Response {
            payload: Some(response::Payload::Findings(proto::Findings {
                total: findings.total,
                selected: findings.selected,
                rows_searched: findings.rows_searched,
                truncated: findings.truncated,
            })),
        },
        Err(reason) => Response::failure(reason),
    }
}

/// Calls a pane what somebody wants to call it.
///
/// Trimmed, and blank reads as taking the name away. A name of spaces is a row that looks
/// empty and cannot be told from an unnamed one, so there is one spelling for "no name"
/// rather than two that render alike.
fn rename_pane(rename: &proto::RenamePane) -> Response {
    let name = wanted_name(&rename.name);
    act(&rename.daemon_id, &rename.pane_id, Keyboard::Follows, |pane| BackendIntent::RenamePane {
        pane,
        name,
    })
}

/// Calls a tab what somebody wants to call it.
///
/// A tab named outright when the caller said which, and otherwise the tab holding the pane
/// the keyboard is on - which is what a chord and a menu item mean, since neither can point
/// at a tab any other way.
fn rename_tab(rename: &proto::RenameTab) -> Response {
    let name = wanted_name(&rename.name);

    if !rename.tab_id.is_empty() {
        // The daemon comes from the tab, the way it comes from a pane in `act`: a tab name is
        // unique across every attached machine, so a caller that has one has said enough.
        let tab = TabId::new(&rename.tab_id);
        let Some(daemon) = holder_of(&tab, &rename.daemon_id) else {
            return no_such_tab(&tab, "renamed");
        };
        return submit(&daemon, &BackendIntent::RenameTab { tab, name }, Keyboard::Follows);
    }

    let daemon = match resolve_daemon(&rename.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return *refusal,
    };
    let Some(pane) = session::focused_pane() else {
        return Response::failure(
            "no pane has this window's keyboard, so there was no tab to rename. A request that \
             names no tab means the one the keyboard is in, and this window has no keyboard - \
             the attach failed earlier, or the pane it succeeded on exited.",
        );
    };
    let Some(tab) = session::tab_of(&daemon, &pane) else {
        return Response::failure(format!(
            "the daemon {daemon} holds no pane called {pane}, so there is no tab to rename and \
             nothing was changed. Most likely it closed while this was in flight."
        ));
    };
    submit(&daemon, &BackendIntent::RenameTab { tab, name }, Keyboard::Follows)
}

/// What a rename was asking for: a name, or none at all.
fn wanted_name(asked: &str) -> Option<String> {
    let asked = asked.trim();
    (!asked.is_empty()).then(|| asked.to_string())
}

/// One press of a font-size chord, on the pane the keyboard is on.
///
/// A direction rather than a size, matching ToggleSidebar: what the chord means is "one more
/// than whatever I have", and the shell does not hold what it has.
fn adjust_font_size(change: &str) -> Response {
    match FontSizeChange::parse(change) {
        Some(change) => answer(session::adjust_font_size(change)),
        None => Response::failure(format!(
            "the core does not know a font size change called {change:?}, so the text stayed \
             the size it was. Only {} exist; the shell builds this from a fixed set, so this \
             is a bug there.",
            FontSizeChange::READABLE.join(", "),
        )),
    }
}

/// Moves the keyboard by a direction rather than to a named pane.
fn focus_relative(direction: &str) -> Response {
    match Step::parse(direction) {
        Some(step) => answer(session::step(step)),
        None => Response::failure(format!(
            "the core does not know a step called {direction:?}, so the keyboard stayed where \
             it was. Only next, previous, left, right, up and down exist; the shell builds \
             this from a fixed set, so this is a bug there."
        )),
    }
}

/// Puts a divider where a drag left it, named by the turns down to it.
fn set_split_ratio(set: proto::SetSplitRatio) -> Response {
    let tab = TabId::new(set.tab_id);
    let Some(daemon) = holder_of(&tab, &set.daemon_id) else {
        return no_such_tab(&tab, "resized");
    };
    submit(
        &daemon,
        &BackendIntent::SetSplitRatio {
            tab,
            path: set
                .path
                .into_iter()
                .map(|second| if second { Branch::Second } else { Branch::First })
                .collect(),
            ratio: set.ratio,
        },
        Keyboard::Follows,
    )
}

/// One wheel notch or trackpad gesture, scaled by what the config file asked for.
///
/// Addressed rather than focused, which is what separates this from every other input path
/// here. A wheel moves the pane the pointer is over, because reading one agent's output while
/// typing into another is the ordinary case in a window of fifteen - so this never touches the
/// keyboard, and pointing at a pane is not a request to type in it.
fn scroll_pane(scroll: &proto::Scroll) -> Response {
    let Some(direction) = ScrollDirection::parse(&scroll.direction) else {
        return Response::failure(format!(
            "the core does not know a scroll direction called {:?}, so the wheel did nothing. \
             Only up and down exist; the shell builds this from a fixed set, so this is a bug \
             there.",
            scroll.direction
        ));
    };
    let daemon = match resolve_daemon(&scroll.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return *refusal,
    };
    let pane = PaneId::new(&scroll.pane_id);
    let Some(attached) = session::attached_pane(&daemon, &pane) else {
        // Not a refusal, and the difference is the point: the pointer being somewhere is not a
        // request, so a wheel over a pane whose bridge has not finished starting should cost
        // nothing and say nothing loud. A `Response::failure` here would be logged as
        // `core.refused` at error level, once per wheel event, for a state that resolves in
        // milliseconds.
        log::debug(
            "scroll.unattached",
            fields! {
                "daemon" => daemon.to_string(),
                "pane" => pane.to_string(),
                "impact" => "the wheel moved nothing. Expected while a pane's bridge is \
                             starting; a pane that never scrolls has no channel at all.",
            },
        );
        return Response::ok();
    };
    attached.input.scroll(direction, lines(scroll.delta));
    Response::ok()
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
fn act(
    daemon_id: &str,
    pane_id: &str,
    keyboard: Keyboard,
    build: impl FnOnce(PaneId) -> BackendIntent,
) -> Response {
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
    // The daemon comes from the pane when a request named one and no daemon. A pane name is
    // Muster's own and unique across every attached machine, so it already says which - and
    // asking a caller to name the machine too would mean a CLI could not reach a devenv pane
    // without first working out where it lives.
    let daemon = match session::daemon_holding(&pane) {
        Some(daemon) if daemon_id.is_empty() => daemon,
        _ => match resolve_daemon(daemon_id) {
            Ok(daemon) => daemon,
            Err(refusal) => return *refusal,
        },
    };
    submit(&daemon, &build(pane), keyboard)
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
    let cwd = (!create.cwd.is_empty()).then(|| create.cwd.clone());
    let run = (!create.run.is_empty()).then(|| create.run.clone());
    let name = (!create.name.is_empty()).then(|| create.name.clone());
    let keyboard = if create.take_focus { Keyboard::Follows } else { Keyboard::StaysPut };
    let named = (!create.pane_id.is_empty()).then(|| PaneId::new(&create.pane_id));
    let Some(pane) = named.or_else(session::focused_pane) else {
        return open_a_workspace(cwd, run, name);
    };
    let daemon = match resolve_daemon(&create.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return *refusal,
    };

    let Some((workspace, inherited)) = session::workspace_of(&daemon, &pane) else {
        return Response::failure(format!(
            "the daemon {daemon} holds no pane called {pane}, so there is no workspace to put \
             a tab in and nothing was made. Most likely it closed while this was in flight."
        ));
    };
    submit(
        &daemon,
        &BackendIntent::CreateTab { workspace, cwd: cwd.or(inherited), run, name },
        keyboard,
    )
}

/// Makes a workspace, when there is no pane to put a tab beside.
///
/// What asking for a tab means in a window showing nothing - every pane closed, or the
/// daemons hold none yet. A tab lives in a workspace and a workspace is named by a pane in
/// it, so a window with none has nothing to name and a workspace is the only request that
/// can produce a pane. Without this a window that empties is a window nobody can refill:
/// every other action is about a pane, and there is no pane.
///
/// The first local daemon, and any daemon at all when none of them is local. Launch makes
/// the same choice and stops at local, because filling an empty window is Muster's own idea
/// and making things on somebody else's machine uninvited is a bigger claim - but pressing
/// the key is the invitation, so the rule that guards launch has nothing to guard here.
///
/// The keyboard follows regardless of what `take_focus` asked for, because this is the one
/// case where it has nowhere else to be. `take_focus` is false so that an agent making panes
/// does not drag somebody's cursor around; here there is no cursor to drag and no other pane
/// to leave it on, and honouring the flag would show a pane nobody can type into until they
/// click it.
fn open_a_workspace(cwd: Option<String>, run: Option<String>, name: Option<String>) -> Response {
    let Some(daemon) = session::first_local_daemon().or_else(session::first_attached_daemon) else {
        return Response::failure(
            "this window has no pane to put a tab beside and no daemon to make one on, so \
             nothing was opened. A window with nothing attached looks like this, and so does \
             the renderer check - the daemon.unavailable records above say which, if a \
             daemon was meant to be there.",
        );
    };
    submit(&daemon, &BackendIntent::CreateWorkspace { cwd, run, name }, Keyboard::Follows)
}

/// The daemon a request means, given what it named.
///
/// Empty is the ordinary case rather than an omission: every menu item sends it, because a
/// menu item is about whatever is in front of the user. Only a window with nothing attached
/// has no answer, and that is the renderer check.
/// Boxed refusal, because this is on the scroll path: a wheel resolves a daemon for every
/// notch, and a `Response` is large enough that returning one by value costs more than the
/// refusal it almost never is.
fn resolve_daemon(daemon_id: &str) -> Result<DaemonId, Box<Response>> {
    if !daemon_id.is_empty() {
        return Ok(DaemonId::new(daemon_id));
    }
    session::focused_daemon().ok_or_else(|| {
        Box::new(Response::failure(
            "this window has no daemon its keyboard is on, so a request that named none had \
             nothing to act on. A window with nothing attached looks like this, and so does \
             one whose every region closed.",
        ))
    })
}

/// What Muster should look like, as far as the config file said anything about it.
///
/// Absent throughout means the renderer's own default rather than one invented here. That is
/// the honest answer for every field: only the shell knows what a separator looks like on this
/// OS, only the machine knows what monospace fonts it has, and a palette written down in the
/// core would be a transcription of somebody else's rather than a decision.
fn read_appearance() -> Response {
    Response { payload: Some(response::Payload::Appearance(Box::new(appearance_message()))) }
}

/// Where the window should open, given the screens the shell found.
///
/// Read from the file rather than from the session, because this is asked during launch and the
/// session's own presentation is not restored until `OpenWindow`. One reader either way: the
/// restore uses the same function a moment later.
fn read_window_frame(screens: &[proto::WindowRect]) -> Response {
    let presentation = session::saved_presentation();
    let screens: Vec<Frame> = screens.iter().copied().map(read_rect).collect();
    Response {
        payload: Some(response::Payload::WindowFrame(proto::WindowFrame {
            rect: presentation.frame.map(|frame| write_rect(frame.fitted(&screens))),
            full_screen: presentation.full_screen,
        })),
    }
}

fn read_rect(rect: proto::WindowRect) -> Frame {
    Frame { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
}

fn write_rect(frame: Frame) -> proto::WindowRect {
    proto::WindowRect { x: frame.x, y: frame.y, width: frame.width, height: frame.height }
}

/// What the machine had to say about the font family the config named.
///
/// The shell looks it up because only a shell can, and the words are decided here because a
/// rule nobody can reach from a test is a rule that drifts (`muster_core::font`).
fn report_font_family(report: &proto::ReportFontFamily) -> Response {
    // A lookup takes long enough for a second save to land while it is out, and the answer to
    // the older question would raise a problem about a family nobody has configured any more.
    // The reload that changed it reports again, so nothing is lost by ignoring this one.
    let configured = session::appearance().font.family.unwrap_or_default();
    if configured != report.family {
        log::info(
            "font.report.stale",
            fields! {
                "reported" => report.family.clone(),
                "configured" => configured,
                "impact" => "nothing - the config changed while this lookup was out, and the \
                             read that changed it reports its own answer",
            },
        );
        return Response::ok();
    }

    let report = FontReport {
        family: report.family.clone(),
        found: report.found,
        monospaced: report.monospaced,
    };
    match font::problem(&report) {
        Some(problem) => {
            log::warn(
                "font.family.unusable",
                fields! {
                    "family" => report.family.clone(),
                    "found" => report.found,
                    "detail" => problem.detail.clone(),
                },
            );
            session::raise_problem(&problem.key, problem.severity, &problem.detail);
        }
        None => session::clear_problem(font::KEY),
    }
    Response::ok()
}

/// What Muster should look like, as the config file left it.
///
/// One builder for the read and the event, on the same terms as `bindings_message`.
fn appearance_message() -> proto::Appearance {
    let appearance = session::appearance();
    let color = |value: Option<config::Rgb>| value.map(|c| c.to_string()).unwrap_or_default();

    proto::Appearance {
        font_family: appearance.font.family.unwrap_or_default(),
        font_size: appearance.font.size.unwrap_or_default(),

        background: color(appearance.colors.background),
        foreground: color(appearance.colors.foreground),
        cursor: color(appearance.colors.cursor),
        cursor_text: color(appearance.colors.cursor_text),
        selection_background: color(appearance.colors.selection_background),
        selection_foreground: color(appearance.colors.selection_foreground),
        palette: appearance
            .colors
            .palette
            .map(|entries| entries.iter().map(ToString::to_string).collect())
            .unwrap_or_default(),

        cursor_style: appearance.cursor.style.map(CursorStyle::as_str).unwrap_or_default().into(),
        cursor_blink: appearance.cursor.blink,
        pane_padding: appearance.pane_padding.map(u32::from),

        divider_color: color(appearance.colors.divider),
    }
}

/// How many lines one scroll gesture is worth.
///
/// The device's delta, scaled by what the config file asked for and rounded up to at least
/// one - a gesture small enough to round to zero is still a gesture somebody made, and a
/// wheel that sometimes does nothing reads as a broken wheel rather than as a small notch.
/// Here rather than in the shell because it is a decision, and a decision in the shell is
/// one no test can reach (`docs/testing.md`, thin shell).
fn lines(delta: f64) -> u16 {
    let scaled = delta.abs() * session::feel().scroll_multiplier;
    if !scaled.is_finite() {
        return 1;
    }
    // Clamped before the cast rather than after, so nothing is ever converted out of range.
    // Truncation is what `as` does to a float past the ceiling, and truncation of a scroll
    // wraps an enormous gesture to a tiny one - a wheel that goes the wrong distance for no
    // visible reason. After the clamp the value is a whole number in `1..=u16::MAX`, so the
    // cast is exact and the lint has nothing left to warn about.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lines = scaled.round().clamp(1.0, f64::from(u16::MAX)) as u16;
    lines
}

/// Moves the keyboard one tab along the window's tab order.
fn step_tab(direction: &str) -> Response {
    match TabStep::parse(direction) {
        Some(direction) => answer(session::step_tab(direction)),
        None => Response::failure(format!(
            "the core does not know a tab step called {direction:?}, so the keyboard stayed \
             where it was. Only next and previous exist - tabs are a list rather than an \
             arrangement, so nothing is to the left of a tab. The shell builds this from a \
             fixed set, so this is a bug there."
        )),
    }
}

/// Puts one pane where another one is, which is what dropping a row on a row means.
fn arrange_pane(arrange: &proto::ArrangePane) -> Response {
    if arrange.pane_id.is_empty() || arrange.onto_pane_id.is_empty() {
        return Response::failure(
            "a pane was asked to move without naming both ends of the move, so nothing was \
             rearranged. Unlike most verbs here there is no 'the focused one' to fall back to: \
             a move names two panes by definition, and whatever built this request dropped one.",
        );
    }
    if arrange.pane_id == arrange.onto_pane_id {
        // Not a refusal. Dropping a row on itself is a gesture somebody makes by accident, and
        // telling them off for it would put a message in the log for a mistake with no cost.
        return Response::ok();
    }
    let pane = PaneId::new(&arrange.pane_id);
    // The daemon from the pane when nobody said, like every other request that names one. A
    // drag always says, because the row it started on knows; a CLI has only the two names, and
    // a name is meant to be a complete address.
    let Some(daemon) = pane_holder(&pane, &arrange.daemon_id) else {
        return Response::failure(format!(
            "no daemon this window is following holds a pane called {pane}, so nothing was \
             rearranged. Either it closed while this was in flight, or the name came from an \
             older window - `muster window` lists the panes this one has."
        ));
    };
    answer(session::arrange_pane(&daemon, &pane, &PaneId::new(&arrange.onto_pane_id)))
}

/// Brings a named tab on screen, which is what clicking its caption means.
///
/// The daemon is optional and found from the tab when it is absent, because a tab name is
/// Muster's own and unique across every attached machine - so a script that read one out of
/// `muster window` has said enough. The tab itself is not optional: unlike a pane, a tab has no
/// "the focused one" to fall back to, and a request naming neither is a caller that had a tab in
/// hand and dropped it.
fn focus_tab(daemon_id: &str, tab_id: &str) -> Response {
    if tab_id.is_empty() {
        return Response::failure(
            "a tab was asked for without naming one, so the keyboard stayed where it was. \
             Unlike a pane, a tab has no 'the focused one' to fall back to - whatever built \
             this request had a tab in hand and dropped it.",
        );
    }
    let tab = TabId::new(tab_id);
    let Some(daemon) = holder_of(&tab, daemon_id) else {
        return no_such_tab(&tab, "focused");
    };
    answer(session::focus_tab(&daemon, &tab))
}

/// Which daemon a request about this pane goes to: the one it named, or the one holding the
/// pane.
///
/// The twin of [`holder_of`] below. Distinct from [`resolve_daemon`], which reads an empty
/// daemon as the focused region's - right for a request about whatever is in front of the
/// user, wrong for one that names a pane somewhere else.
fn pane_holder(pane: &PaneId, daemon_id: &str) -> Option<DaemonId> {
    if daemon_id.is_empty() {
        return session::daemon_holding(pane);
    }
    Some(DaemonId::new(daemon_id))
}

/// Which daemon a request about this tab goes to: the one it named, or the one holding the tab.
fn holder_of(tab: &TabId, daemon_id: &str) -> Option<DaemonId> {
    if daemon_id.is_empty() {
        return session::daemon_holding_tab(tab);
    }
    Some(DaemonId::new(daemon_id))
}

/// Why a tab name went nowhere.
///
/// The same answer the registry gives for a name it has dropped, and it means the same thing:
/// whoever said this name is talking about a tab that is not there. Refused rather than sent to
/// whichever daemon happens to be focused, because herdr ignores a `tab_id` it does not
/// recognize and acts on what it has focused instead - so a hopeful send would move or rename
/// somebody else's tab and report success.
fn no_such_tab(tab: &TabId, verb: &str) -> Response {
    Response::failure(format!(
        "no daemon this window is following holds a tab called {tab}, so nothing was {verb}. \
         Either it closed while this was in flight, or the name came from an older window - \
         `muster window` lists the tabs this one has."
    ))
}

/// Goes to the pane at a place in the window's pane order, counting from one.
fn focus_pane_at(place: u32) -> Response {
    let Ok(place) = usize::try_from(place) else {
        return Response::failure(format!(
            "a pane was asked for at place {place}, which does not fit this machine's index \
             type. Nothing moved. Places come from the roster and no window holds that many \
             panes, so this is a bug in whatever built the request."
        ));
    };
    if place == 0 {
        return Response::failure(
            "a pane was asked for at place zero, so the keyboard stayed where it was. Places \
             count from one, the way the sidebar numbers them and the way ⌘1 reads - so the \
             shell building this has an off-by-one.",
        );
    }
    answer(session::focus_pane_at(place))
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

/// Asks a daemon for a change, and says what came of it.
///
/// A request that made a pane answers with which, because that is the one fact about what just
/// happened that a caller cannot get any other way: a script's next line names that pane, and
/// its name was minted inside this call. Everything else about the change arrives as a view.
fn submit(daemon: &DaemonId, intent: &BackendIntent, keyboard: Keyboard) -> Response {
    match session::submit(daemon, intent, keyboard) {
        Ok(Some(pane)) => Response {
            payload: Some(response::Payload::Made(proto::Made { pane_id: pane.to_string() })),
        },
        Ok(None) => Response::ok(),
        Err(refusal) => refused(&refusal),
    }
}

fn answer(outcome: Result<(), String>) -> Response {
    match outcome {
        Ok(()) => Response::ok(),
        Err(refusal) => refused(&refusal),
    }
}

fn refused(detail: &str) -> Response {
    Response::failure(format!(
        "the daemon did not make that change: {detail} Nothing about the session moved, and the \
         window still shows what it showed before - which is the honest answer rather than a \
         view that pretends."
    ))
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

/// Every action and the chord asking for it, for a shell to build a menu from.
///
/// Answered from the config rather than from a table in the shell, which is what makes
/// rebinding one thing: a file that moves `split_right` moves the menu item, and on macOS the
/// menu item is the binding.
fn read_bindings() -> Response {
    Response { payload: Some(response::Payload::Bindings(bindings_message())) }
}

/// What this window is showing, what it holds that it is not showing, and how its agents are.
///
/// The read that makes the endpoint worth having. An agent driving a window has no eyes: it can
/// split a pane and start something in it, and without this it can never learn whether either
/// happened. The same four messages the shell is sent as events, from the same builders.
fn read_window() -> Response {
    let now = session::window();
    Response {
        payload: Some(response::Payload::Window(proto::Window {
            view: Some(convert::view(&now.view)),
            roster: Some(convert::roster(&now.roster, &now.numbering)),
            panes: now
                .agents
                .iter()
                .map(|(pane, state)| proto::PaneStateChanged {
                    daemon_id: pane.daemon.to_string(),
                    pane_id: pane.pane.to_string(),
                    state: state.as_str().to_string(),
                })
                .collect(),
            daemons: now
                .daemons
                .iter()
                .map(|(daemon, health, detail)| proto::BackendHealth {
                    daemon_id: daemon.to_string(),
                    state: health.as_str().to_string(),
                    detail: detail.clone(),
                })
                .collect(),
        })),
    }
}

/// Which chord means what, as the config file left it.
///
/// One builder for the read and the event, so the answer a launch gets and the answer a reload
/// sends cannot drift into two shapes.
fn bindings_message() -> proto::Bindings {
    proto::Bindings {
        // An action on no chord is published with an empty key rather than left out. It
        // is still a menu item - a shortcut is not the only way to pick one - and on
        // macOS an action with no item is an action nothing can reach.
        bindings: session::bindings()
            .all()
            .map(|(action, chord)| proto::Binding {
                action: action.as_str().to_string(),
                key: chord.map(|chord| chord.key.as_str().to_string()).unwrap_or_default(),
                modifiers: chord
                    .into_iter()
                    .flat_map(|chord| {
                        Modifiers::ALL_NAMES.into_iter().filter(move |(_, bit)| {
                            Modifiers::CHORD.contains(*bit) && chord.modifiers.contains(*bit)
                        })
                    })
                    .map(|(name, _)| name.to_string())
                    .collect(),
            })
            .collect(),
    }
}

/// Tells the shell the chords moved, which on macOS is what rebuilds the menu.
fn announce_bindings() {
    crate::ffi::emit(&proto::Event {
        payload: Some(event::Payload::BindingsChanged(proto::BindingsChanged {
            bindings: Some(bindings_message()),
        })),
    });
}

/// Tells the shell what the window should look like now.
fn announce_appearance() {
    crate::ffi::emit(&proto::Event {
        payload: Some(event::Payload::AppearanceChanged(proto::AppearanceChanged {
            appearance: Some(appearance_message()),
        })),
    });
}

/// Splits a pane, putting the new one on the named side of it.
fn split_pane(split: &proto::SplitPane) -> Response {
    let Some(side) = Side::parse(&split.side) else {
        return Response::failure(format!(
            "the core does not know a side called {:?}, so nothing was split. They are left, \
             right, up and down; the shell builds this from a fixed set, so this is a bug \
             there.",
            split.side
        ));
    };
    let keyboard = if split.take_focus { Keyboard::Follows } else { Keyboard::StaysPut };
    act(&split.daemon_id, &split.pane_id, keyboard, |pane| BackendIntent::SplitPane {
        pane,
        side,
        // Zero is proto3's unset, and a divider at the very edge is not a thing anyone asks
        // for, so the two are safely the same answer here.
        ratio: (split.ratio > 0.0).then_some(split.ratio),
        // Empty is the daemon's own rule rather than this process's directory, and for a
        // split that rule is "wherever the pane you split was".
        cwd: (!split.cwd.is_empty()).then(|| split.cwd.clone()),
        run: (!split.run.is_empty()).then(|| split.run.clone()),
        name: (!split.name.is_empty()).then(|| split.name.clone()),
    })
}

/// Types text into a pane, named rather than focused.
fn send_to_pane(send: &proto::SendToPane) -> Response {
    // The keyboard never moves for this. Being sent something is not the same as being looked
    // at, and an agent telling two others what to do would otherwise pull the user's cursor
    // onto whichever it addressed last.
    act(&send.daemon_id, &send.pane_id, Keyboard::StaysPut, |pane| BackendIntent::SendText {
        pane,
        text: send.text.clone(),
        enter: send.enter,
    })
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
    // Zero is proto3's unset, and resizing a pane by nothing is not a thing anyone asks for,
    // so the two are safely the same answer. What a keystroke means by "unset" is the config
    // file's `resize_step`, and what that means by absent is the daemon's own step - resolved
    // here rather than in the shell, so a CLI asking for the same thing gets the same answer.
    //
    // Both measurements are taken along the axis the divider travels: a horizontal chord
    // divides by the cell's width and the region's width, a vertical one by their heights. The
    // cell asymmetry is the whole reason points are offered as a unit at all; the region
    // asymmetry is just which way the window happens to be shaped.
    let step = session::feel().resize_step;
    let (cell, extent) = match direction {
        Side::Left | Side::Right => (resize.cell_width, resize.region_width),
        Side::Up | Side::Down => (resize.cell_height, resize.region_height),
    };
    let measured = |value: f32| (value > 0.0).then_some(value);
    let configured = step.and_then(|step| step.fraction(measured(cell), measured(extent)));
    if let Some(step) = step.filter(|_| configured.is_none()) {
        log::warn(
            "config.resizeStep.unmeasured",
            fields! {
                "step" => step,
                "direction" => direction.as_str(),
                "impact" => "this resize moved the daemon's own step instead of the distance the \
                              config file asked for",
                "detail" => "a distance becomes a divider position only against the size of a \
                              cell and the size of the region it is a share of, and this caller \
                              reported one of them as zero. A chord always reports both, so this \
                              is either a surface nothing has measured yet or a caller with no \
                              window at all.",
            },
        );
    }
    let fraction = (resize.amount > 0.0).then_some(resize.amount).or(configured);
    act(&resize.daemon_id, &resize.pane_id, Keyboard::Follows, |pane| BackendIntent::ResizePane {
        pane,
        direction,
        fraction,
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
                backend_pane_id: pane.backend_pane_id.clone(),
            })),
        },
        Err(AttachError::Unreachable(detail)) => Response::failure(format!(
            "{detail} Nothing is known about this pane, so the window will render nothing."
        )),
        // Muster's own answer to "then what is there", rather than the daemon's CLI. Somebody
        // who typed a Muster command and got a refusal should not have to learn which daemon
        // is underneath it to act on one, and the schema-drift half of this belongs to
        // whoever is debugging Muster - it is already a `mirror.entries_dropped` record.
        Err(AttachError::NoSuchPane { pane, held, dropped }) => Response::failure(format!(
            "no pane called {pane} exists, so this window has nothing to show and would \
             render and ignore the keyboard. The attached sessions hold {held} pane(s) \
             between them - run `muster` with no arguments to open onto them and see the \
             list. {dropped} more could not be read, so if this id is one of those, the run \
             log says what went unread."
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
    session::set_daemon_binary(&startup.daemon_path);
    // Before the config too, and for a sharper reason: applying a config attaches daemons,
    // attaching publishes, and a publish before this is one that would write the arrangement
    // out to nowhere - or worse, read it back after it had been replaced.
    session::set_state_path(&startup.state_path);
    // Before the config, and for the sharpest version of the same reason: applying one attaches
    // daemons, and the first snapshot from each mints a name for every pane it describes. Read
    // afterwards, every pane already open would be named a second time, and a program running
    // in one would hold a name for nothing.
    session::set_pane_names_path(&startup.pane_names_path);
    // Before the config too, because applying one can start a daemon and the locale is part of
    // the environment that daemon is born with. Set after, it would reach the second launch.
    session::set_platform_locale(&startup.locale);
    // Before the config for the sharpest version of that reason: this is where the file that
    // daemon reads gets written, and `herdr server` reads its config once at startup.
    session::set_daemon_config_path(&startup.daemon_config_path);
    // Before the config for the same reason once more: applying one can start a daemon, and this
    // is part of the environment that daemon is born with - so a pane it spawns has `muster` on
    // its PATH from the first one onwards.
    session::set_commands_path(&startup.commands_path);
    // Before the config, because applying one can attach a daemon on another machine and that
    // is the whole of what this is for: a herdr for that machine's platform, fetched here and
    // pushed across. Set after, the first launch to meet a new devenv would download to a
    // temporary and throw it away.
    session::set_cache_path(&startup.cache_path);

    if let Err(refusal) = start_logging(startup) {
        return *refusal;
    }
    // After logging, because the one line that says where this window is listening is the first
    // thing anybody reads when the CLI cannot find a window - and bound before the config,
    // because applying one can wait on a daemon starting. A caller that dials during that gets a
    // window with no daemons attached, which is the truth at that moment and says so: the health
    // in the answer is `disconnected` rather than the pane list merely being empty.
    command::listen(&startup.command_socket_path);
    apply_config(&startup.config_path);
    Response::ok()
}

/// Opens this run's log, or says why it could not be opened.
///
/// Separate from [`start`] so that everything after it in a launch is inside the record. An empty
/// path is success with nothing to do: it is what a release build asks for, and what every seam
/// test that names no log gets.
fn start_logging(startup: &proto::Startup) -> Result<(), Box<Response>> {
    if startup.log_path.is_empty() {
        return Ok(());
    }
    let Some(sink) = JsonLinesSink::open(&startup.log_path) else {
        return Err(Box::new(Response::failure(format!(
            "the core could not open {} for logging, so this run leaves no record. \
             Everything else works; a bug report from it will just be missing the timeline \
             that usually explains what happened. Check that the directory exists and is \
             writable.",
            startup.log_path
        ))));
    };
    let level = if startup.log_level.is_empty() {
        LogLevel::Debug
    } else {
        match LogLevel::parse(&startup.log_level) {
            Some(level) => level,
            None => {
                return Err(Box::new(Response::failure(format!(
                    "the core does not know a log level called {:?}, so logging stayed off \
                     and this run leaves no record. Valid levels are trace, debug, info, \
                     warn and error; check MUSTER_LOG_LEVEL.",
                    startup.log_level
                ))));
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
            "daemon_binary" => if startup.daemon_path.is_empty() {
                "(none staged)".to_string()
            } else {
                startup.daemon_path.clone()
            },
        },
    );
    Ok(())
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
    session::set_config_path(path);
    let Some(config) = read_config(path, Reading::Launch) else {
        return;
    };
    log::info(
        "config.read",
        fields! {
            "path" => path.to_string(),
            "daemons" => config.daemons.len().to_string(),
            "option_as_alt" => config.input.option_as_alt.as_str(),
            "text_bindings" => config.input.text.len().to_string(),
        },
    );
    session::set_bindings(config.bindings.clone());
    session::set_pane_input(config.input.clone());
    session::set_feel(config.feel);
    session::set_appearance(config.appearance.clone());
    // Before following, which is what writes the derived config and starts a daemon
    // that reads it. Set after, a first launch would give its daemon last launch's
    // answer about what a pane runs.
    session::set_panes(config.panes.clone());
    session::set_configured_daemons(&config.daemons);
    session::follow_configured(&config);
}

/// The one problem key a config file can raise.
///
/// One rather than one per way of failing, because only one of them can be true at a time and
/// every one of them is fixed by the same act - editing the file until Muster accepts it. Two
/// keys would mean each read had to remember to clear the other's, which is a thing to forget.
const CONFIG_PROBLEM: &str = "config";

/// Which of the two moments is reading the file.
///
/// The moments differ in what a failure means for the window, and that difference is real
/// enough to be worth saying in the log: at launch no configured daemon gets attached, on a
/// save nothing changes at all. Everything else about failing is the same.
#[derive(Clone, Copy)]
enum Reading {
    Launch,
    Reload,
}

impl Reading {
    fn event(self, kind: &str) -> String {
        match self {
            Reading::Launch => format!("config.{kind}"),
            Reading::Reload => format!("config.reload.{kind}"),
        }
    }

    /// What the window is left doing, which is the line an investigator reads first.
    fn impact(self) -> &'static str {
        match self {
            Reading::Launch => {
                "no daemon named in that file is attached, so the window shows only the daemon \
                 on this machine"
            }
            Reading::Reload => {
                "nothing changed; the window is still running the settings it started with, so \
                 this is a file to fix rather than a window to restart"
            }
        }
    }
}

/// Reads the config file, or says why not - and tells the person either way.
///
/// Shared by the launch and the reload because they have to fail identically. They used not
/// to: there were two copies of this parse, four log event names between them and two
/// hand-written impact lines, so anything either moment learned to do about a bad file had to
/// be learned twice. Raising a problem is exactly such a thing, and it would have landed in
/// one of them.
///
/// Every exit clears or raises `CONFIG_PROBLEM`, which is what makes the window's answer
/// track the file rather than track whichever read happened last. A file fixed after a
/// refusal clears it here, and that disappearance is the only confirmation anybody gets that
/// a save was accepted.
fn read_config(path: &str, reading: Reading) -> Option<config::Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            let detail = format!(
                "Muster could not read its config file at {path}: {error}. It is running on \
                 defaults until that file can be read. Check that the path exists and is \
                 readable."
            );
            log::warn(
                &reading.event("unreadable"),
                fields! {
                    "path" => path.to_string(),
                    "detail" => error.to_string(),
                    "impact" => reading.impact(),
                    "check" => "whether the path exists and is readable; the shell only sends \
                                one it has already seen",
                },
            );
            session::raise_problem(CONFIG_PROBLEM, Severity::Error, &detail);
            return None;
        }
    };
    match config::parse(&text) {
        Ok(config) => {
            session::clear_problem(CONFIG_PROBLEM);
            Some(config)
        }
        Err(refusal) => {
            log::warn(
                &reading.event("refused"),
                fields! {
                    "path" => path.to_string(),
                    "detail" => refusal.clone(),
                    "impact" => reading.impact(),
                },
            );
            // The refusal, whole and unedited. `config.rs` writes these to be read by whoever
            // caused them - they name the value, what stopped working and what to type instead
            // - so anything composed here would be a worse sentence about the same fact.
            session::raise_problem(CONFIG_PROBLEM, Severity::Error, &refusal);
            None
        }
    }
}

/// Reads the config file again, and makes the window match it.
///
/// What a relaunch used to be for. Everything a file can say takes effect except which daemons
/// are attached: attaching and detaching on a file save is a question about live sessions rather
/// than about settings, and getting it wrong costs somebody their panes. So a `[[daemon]]` change
/// is read, noticed, and reported as still needing a relaunch.
///
/// A refusal leaves the running configuration exactly as it was, which is the same
/// whole-or-nothing rule the file already has, one level up: the alternative is a window running
/// half of a file somebody is still editing.
fn reload_config() -> Response {
    let path = session::config_path();
    if path.is_empty() {
        log::info("config.reload.none", fields! {});
        return Response::ok();
    }

    let Some(config) = read_config(&path, Reading::Reload) else {
        return Response::ok();
    };

    // Named before anything is applied, because it is the one thing a reload cannot do and the
    // person who just edited that block is about to wonder why nothing happened.
    if session::daemons_differ(&config) {
        log::warn(
            "config.reload.daemons",
            fields! {
                "path" => path.clone(),
                "impact" => "every other setting in the file took effect, but which daemons \
                             this window is attached to did not",
                "check" => "relaunch to pick up a [[daemon]] change; attaching and detaching \
                            live would move panes somebody is working in",
            },
        );
    }

    session::set_bindings(config.bindings.clone());
    session::set_feel(config.feel);
    session::set_appearance(config.appearance.clone());
    session::set_panes(config.panes.clone());
    // Unlike `[[daemon]]`, this one is not left for a relaunch, because a relaunch would not
    // fix it: the daemon is started and never stopped, so it outlives every launch and would
    // go on running the settings it was born with until the machine was rebooted. Rewriting
    // the file and asking the daemon to read it again is what makes saving the file mean
    // something - as far as it can go, which is panes opened from now on.
    session::rewrite_daemon_configuration();
    // Recorded even though it is not acted on, so the next reload compares against this file
    // rather than reporting the same unapplied change forever.
    session::set_configured_daemons(&config.daemons);
    // Last of the four, because it is the one that reaches into panes that already exist.
    session::reset_pane_input(&config.input);

    log::info(
        "config.reload.read",
        fields! {
            "path" => path,
            "option_as_alt" => config.input.option_as_alt.as_str(),
            "text_bindings" => config.input.text.len().to_string(),
        },
    );
    announce_bindings();
    announce_appearance();
    // The roster too, because `numbered_chords` decides which rows carry a number and the
    // sidebar is drawing them. Without this, saving a file that changes the scheme moves what
    // the chords do and leaves the numbers beside the rows saying what they used to do - which
    // is the one failure the numbers are drawn to prevent.
    //
    // Announced here rather than left to the next publish, and the difference is the whole
    // point: a reload asks the daemon to re-read its own config, so it says something shortly
    // afterwards and the roster is republished anyway. The numbers would come right either
    // way - by luck, a moment later. What has to be true is that they are right when the save
    // returns.
    session::announce_roster();
    Response::ok()
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
