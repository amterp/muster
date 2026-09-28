//! An input connection: keystrokes, clicks, pastes and `pane send` text, never answered.
//!
//! Each event names its pane and goes to that pane's writer queue, which encodes it against
//! the pane's modes (`writer.rs`). The connection never waits on a pane: an event for a pane
//! whose program has stopped reading, with its queue full, is dropped rather than held, since
//! holding it would stall every other pane's input behind one program.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Weak};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_core::input::{KeyAction, Modifiers, OptionAsAlt};
use muster_daemon_proto::connection;
use muster_daemon_proto::{self as proto, input_event, input_event::perform};
use muster_vt::{MouseAction, MouseButton, MouseEvent};

use crate::pane::PaneIo;
use crate::session::Shared;
use crate::writer::{Input, OwnedKey, Wheel};

/// Serves a welcomed input connection until it hangs up.
pub(crate) fn serve(mut stream: UnixStream, shared: &Arc<Shared>, client: &str) {
    log::info("daemon.input.opened", fields! { "client" => client });
    crate::priority::interactive();
    let mut panes = Panes::default();
    // Panes whose queue was full at the last event, so a stall is said once rather than per
    // keystroke.
    let mut stalled: Vec<String> = Vec::new();
    loop {
        let event = match connection::receive::<proto::InputEvent>(&mut stream) {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => {
                log::warn(
                    "daemon.input.unreadable",
                    fields! {
                        "client" => client,
                        "error" => error,
                        "impact" => "the input connection is closed; its client has to reconnect",
                        "check" => "whether the client speaks this daemon's protocol version",
                    },
                );
                break;
            }
        };
        let Some(input) = event.input.and_then(input_of) else {
            continue;
        };
        let Some(io) = panes.get(&event.pane, || shared.lock().pane_io(&event.pane)) else {
            log::debug("daemon.input.no_pane", fields! { "pane" => event.pane });
            continue;
        };
        if io.queue(input) {
            stalled.retain(|pane| *pane != event.pane);
        } else if !stalled.contains(&event.pane) {
            log::warn(
                "daemon.input.dropped",
                fields! {
                    "pane" => event.pane,
                    "impact" => "input for this pane is dropped until its program reads again",
                    "check" => "whether the pane's program has stopped reading its terminal",
                },
            );
            stalled.push(event.pane);
        }
    }
    log::info("daemon.input.closed", fields! { "client" => client });
}

/// The panes a connection has sent input to, by name, so that a keystroke does not take the
/// session lock. Held weakly: a connection keeps no pane alive.
#[derive(Default)]
struct Panes {
    cached: HashMap<String, Weak<PaneIo>>,
}

impl Panes {
    /// The pane named `name`, from the cache, else from `look_up`.
    fn get(
        &mut self,
        name: &str,
        look_up: impl FnOnce() -> Option<Arc<PaneIo>>,
    ) -> Option<Arc<PaneIo>> {
        // A closed pane's name may already be another pane's.
        if let Some(io) = self.cached.get(name).and_then(Weak::upgrade).filter(|io| !io.is_closed())
        {
            return Some(io);
        }
        let io = look_up()?;
        self.cached.insert(name.to_string(), Arc::downgrade(&io));
        Some(io)
    }
}

/// What an event asks the pane's writer for. Nothing for an event that names nothing to do.
pub(crate) fn input_of(input: input_event::Input) -> Option<Input> {
    use input_event::Input as Event;
    let modifiers = |bits: u32| Modifiers(u16::try_from(bits).unwrap_or(0));
    match input {
        Event::Key(key) => Some(Input::Key(owned_key(key))),
        Event::Mouse(mouse) => {
            let action = match mouse.action() {
                proto::MouseAction::Press => MouseAction::Press,
                proto::MouseAction::Release => MouseAction::Release,
                proto::MouseAction::Motion => MouseAction::Motion,
                proto::MouseAction::Unspecified => return None,
            };
            let button = match mouse.button() {
                proto::MouseButton::Left => Some(MouseButton::Left),
                proto::MouseButton::Right => Some(MouseButton::Right),
                proto::MouseButton::Middle => Some(MouseButton::Middle),
                proto::MouseButton::None | proto::MouseButton::Unspecified => None,
            };
            Some(Input::Mouse(MouseEvent {
                action,
                button,
                modifiers: modifiers(mouse.mods),
                position: position(mouse.x, mouse.y),
            }))
        }
        Event::Wheel(wheel) => Some(Input::Wheel(Wheel {
            dx: wheel.dx,
            dy: wheel.dy,
            precise: wheel.precise,
            modifiers: modifiers(wheel.mods),
            position: position(wheel.x, wheel.y),
        })),
        Event::Paste(paste) => Some(Input::Paste { text: paste.text, confirmed: paste.confirmed }),
        Event::Send(send) => Some(Input::Send { text: send.text, enter: send.enter }),
        Event::Focus(focus) => Some(Input::Focus(focus.focused)),
        Event::Perform(input_event::Perform { action: Some(action), key }) => match action {
            perform::Action::Raw(bytes) => Some(Input::Bound(bytes)),
            perform::Action::Reset(_) => Some(Input::Reset),
            perform::Action::ClearScreen(_) => Some(Input::ClearScreen { key: key.map(owned_key) }),
        },
        Event::Perform(input_event::Perform { action: None, .. }) => None,
    }
}

fn owned_key(key: input_event::Key) -> OwnedKey {
    OwnedKey {
        action: match key.action() {
            proto::KeyAction::Release => KeyAction::Release,
            proto::KeyAction::Repeat => KeyAction::Repeated,
            proto::KeyAction::Press | proto::KeyAction::Unspecified => KeyAction::Press,
        },
        code: key.key,
        modifiers: u16::try_from(key.mods).unwrap_or(0),
        consumed_modifiers: u16::try_from(key.consumed_mods).unwrap_or(0),
        unshifted_codepoint: key.unshifted_codepoint,
        composing: key.composing,
        option_as_alt: match key.option_as_alt() {
            proto::OptionAsAlt::Always => OptionAsAlt::Always,
            proto::OptionAsAlt::Left => OptionAsAlt::LeftOnly,
            proto::OptionAsAlt::Right => OptionAsAlt::RightOnly,
            proto::OptionAsAlt::Never | proto::OptionAsAlt::Unspecified => OptionAsAlt::Never,
        },
        text: key.text,
    }
}

/// A surface position in pixels, which never needs more than f32 holds.
#[allow(clippy::cast_possible_truncation)]
fn position(x: f64, y: f64) -> (f32, f32) {
    (x as f32, y as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_whose_pane_closed_is_looked_up_again() {
        let old = PaneIo::idle(1);
        let new = PaneIo::idle(2);
        let mut panes = Panes::default();
        assert_eq!(panes.get("p", || Some(Arc::clone(&old))).map(|io| io.serial), Some(1));
        assert_eq!(panes.get("p", || None).map(|io| io.serial), Some(1), "from the cache");
        old.mark_closed();
        let found = panes.get("p", || Some(Arc::clone(&new)));
        assert_eq!(found.map(|io| io.serial), Some(2), "the pane now of that name");
    }
}
