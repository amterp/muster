//! What each request means.
//!
//! Bytes in, bytes out, no FFI: this is where the seam's behavior can be tested without a
//! shell, a window or a linker. [`crate::ffi`] is the shim that lets a C caller reach it.

use std::collections::BTreeMap;

use muster_core::diagnostics::log::{self, LogLevel};
use muster_core::diagnostics::sink::JsonLinesSink;
use muster_core::fields;

use muster_core::composition::{DaemonId, FontSizeChange, RegionId, Step};
use muster_core::config::{self, CursorStyle};
use muster_core::input::{CompositionOutcome, Modifiers, ScrollDirection, composition_outcome};
use muster_core::intent::{BackendIntent, Branch, Side};
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::roster::TabStep;

use crate::convert;
use crate::proto::{self, Request, Response, event, request, response};
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
        request::Payload::SplitPane(split) => split_pane(&split),
        request::Payload::ClosePane(close) => {
            act(&close.daemon_id, &close.pane_id, |pane| BackendIntent::ClosePane { pane })
        }
        request::Payload::ReadBindings(_) => read_bindings(),
        request::Payload::ReadAppearance(_) => read_appearance(),
        request::Payload::ResizePane(resize) => resize_pane(&resize),
        request::Payload::ToggleSidebar(_) => {
            session::toggle_sidebar();
            Response::ok()
        }
        request::Payload::AdjustFontSize(adjust) => adjust_font_size(&adjust.change),
        request::Payload::ReloadConfig(_) => reload_config(),
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
        request::Payload::FocusRelative(step) => focus_relative(&step.direction),
        request::Payload::FocusTabRelative(step) => step_tab(&step.direction),
        request::Payload::FocusTabAt(at) => focus_tab_at(at.place),
        request::Payload::SetSplitRatio(set) => set_split_ratio(set),
        request::Payload::Scroll(scroll) => scroll_pane(&scroll),
        request::Payload::RenamePane(rename) => rename_pane(&rename),
        request::Payload::RenameTab(rename) => rename_tab(&rename),
    }
}

/// Calls a pane what somebody wants to call it.
///
/// Trimmed, and blank reads as taking the name away. A name of spaces is a row that looks
/// empty and cannot be told from an unnamed one, so there is one spelling for "no name"
/// rather than two that render alike.
fn rename_pane(rename: &proto::RenamePane) -> Response {
    let name = wanted_name(&rename.name);
    act(&rename.daemon_id, &rename.pane_id, |pane| BackendIntent::RenamePane { pane, name })
}

/// Calls a tab what somebody wants to call it.
///
/// A tab named outright when the caller said which, and otherwise the tab holding the pane
/// the keyboard is on - which is what a chord and a menu item mean, since neither can point
/// at a tab any other way.
fn rename_tab(rename: &proto::RenameTab) -> Response {
    let daemon = match resolve_daemon(&rename.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return refusal,
    };
    let name = wanted_name(&rename.name);

    if !rename.tab_id.is_empty() {
        return submit(
            &daemon,
            &BackendIntent::RenameTab { tab: TabId::new(&rename.tab_id), name },
        );
    }
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
    submit(&daemon, &BackendIntent::RenameTab { tab, name })
}

/// What a rename was asking for: a name, or none at all.
fn wanted_name(asked: &str) -> Option<String> {
    let asked = asked.trim();
    (!asked.is_empty()).then(|| asked.to_string())
}

/// One press of a font-size chord.
///
/// A direction rather than a size, matching ToggleSidebar: what the chord means is "one more
/// than whatever I have", and the shell does not hold what it has.
fn adjust_font_size(change: &str) -> Response {
    match FontSizeChange::parse(change) {
        Some(change) => {
            session::adjust_font_size(change);
            Response::ok()
        }
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
    match resolve_daemon(&set.daemon_id) {
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
    }
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
        Err(refusal) => return refusal,
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
    let cwd = (!create.cwd.is_empty()).then(|| create.cwd.clone());
    let named = (!create.pane_id.is_empty()).then(|| PaneId::new(&create.pane_id));
    let Some(pane) = named.or_else(session::focused_pane) else {
        return open_a_workspace(cwd);
    };
    let daemon = match resolve_daemon(&create.daemon_id) {
        Ok(daemon) => daemon,
        Err(refusal) => return refusal,
    };

    let Some((workspace, inherited)) = session::workspace_of(&daemon, &pane) else {
        return Response::failure(format!(
            "the daemon {daemon} holds no pane called {pane}, so there is no workspace to put \
             a tab in and nothing was made. Most likely it closed while this was in flight."
        ));
    };
    submit(&daemon, &BackendIntent::CreateTab { workspace, cwd: cwd.or(inherited) })
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
fn open_a_workspace(cwd: Option<String>) -> Response {
    let Some(daemon) = session::first_local_daemon().or_else(session::first_attached_daemon) else {
        return Response::failure(
            "this window has no pane to put a tab beside and no daemon to make one on, so \
             nothing was opened. A window with nothing attached looks like this, and so does \
             the renderer check - the daemon.unavailable records above say which, if a \
             daemon was meant to be there.",
        );
    };
    submit(&daemon, &BackendIntent::CreateWorkspace { cwd })
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

/// What Muster should look like, as far as the config file said anything about it.
///
/// Absent throughout means the renderer's own default rather than one invented here. That is
/// the honest answer for every field: only the shell knows what a separator looks like on this
/// OS, only the machine knows what monospace fonts it has, and a palette written down in the
/// core would be a transcription of somebody else's rather than a decision.
fn read_appearance() -> Response {
    Response { payload: Some(response::Payload::Appearance(Box::new(appearance_message()))) }
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

/// Shows the tab at a place in the window's tab order, counting from one.
fn focus_tab_at(place: u32) -> Response {
    let Ok(place) = usize::try_from(place) else {
        return Response::failure(format!(
            "a tab was asked for at place {place}, which does not fit this machine's index \
             type. Nothing moved. Places come from the roster and no window holds that many \
             tabs, so this is a bug in whatever built the request."
        ));
    };
    if place == 0 {
        return Response::failure(
            "a tab was asked for at place zero, so the keyboard stayed where it was. Places \
             count from one, the way the sidebar lists them and the way ⌘1 reads - so the \
             shell building this has an off-by-one.",
        );
    }
    answer(session::focus_tab_at(place))
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

/// Every action and the chord asking for it, for a shell to build a menu from.
///
/// Answered from the config rather than from a table in the shell, which is what makes
/// rebinding one thing: a file that moves `split_right` moves the menu item, and on macOS the
/// menu item is the binding.
fn read_bindings() -> Response {
    Response { payload: Some(response::Payload::Bindings(bindings_message())) }
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
    act(&split.daemon_id, &split.pane_id, |pane| BackendIntent::SplitPane {
        pane,
        side,
        // Zero is proto3's unset, and a divider at the very edge is not a thing anyone asks
        // for, so the two are safely the same answer here.
        ratio: (split.ratio > 0.0).then_some(split.ratio),
        // Empty is the daemon's own rule rather than this process's directory, and for a
        // split that rule is "wherever the pane you split was".
        cwd: (!split.cwd.is_empty()).then(|| split.cwd.clone()),
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
    // Widened here rather than stored wide: the seam's field is a float so a CLI can place a
    // divider exactly, and a chord's step is whole cells. Every u16 is a float exactly.
    let amount = (resize.amount > 0.0)
        .then_some(resize.amount)
        .or_else(|| session::feel().resize_step.map(f32::from));
    act(&resize.daemon_id, &resize.pane_id, |pane| BackendIntent::ResizePane {
        pane,
        direction,
        amount,
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
    session::set_daemon_binary(&startup.herdr_path);
    // Before the config too, and for a sharper reason: applying a config attaches daemons,
    // attaching publishes, and a publish before this is one that would write the arrangement
    // out to nowhere - or worse, read it back after it had been replaced.
    session::set_state_path(&startup.state_path);
    // Before the config too, because applying one can start a daemon and the locale is part of
    // the environment that daemon is born with. Set after, it would reach the second launch.
    session::set_platform_locale(&startup.locale);

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
    session::set_config_path(path);
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
            session::set_configured_daemons(&config.daemons);
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

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            log::warn(
                "config.reload.unreadable",
                fields! {
                    "path" => path.clone(),
                    "detail" => error.to_string(),
                    "impact" => "nothing changed; the window is still running the settings it \
                                 started with",
                    "check" => "whether the file still exists - it was readable at launch",
                },
            );
            return Response::ok();
        }
    };

    let config = match config::parse(&text) {
        Ok(config) => config,
        Err(refusal) => {
            log::warn(
                "config.reload.refused",
                fields! {
                    "path" => path.clone(),
                    "detail" => refusal,
                    "impact" => "nothing changed; the window is still running the settings it \
                                 started with, so this is a file to fix rather than a window to \
                                 restart",
                },
            );
            return Response::ok();
        }
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
