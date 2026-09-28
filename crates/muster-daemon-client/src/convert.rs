//! The daemon's protocol in the core's words, and back.
//!
//! The one place the two vocabularies meet, so the core depends on no daemon crate (MIP-3,
//! section 11) and a change to either side is a change here.

use muster_core::AgentState;
use muster_core::config::{CursorStyle, Rgb, ShellMode};
use muster_core::daemon_settings::{DaemonSettings, Palette};
use muster_core::input::{InputEvent, KeyAction, MouseAction, MouseButton, OptionAsAlt};
use muster_core::intent::{Branch, Side};
use muster_core::mirror::backend::{AgentFacts, LayoutNode, Pane, PaneId, SplitAxis, Tab, TabId};
use muster_core::mirror::{BackendEvent, Restored, Snapshot};
use muster_daemon_proto::{self as proto, event, input_event, pane_effect};

/// A snapshot, with any tab whose tree will not read left out and counted.
pub fn snapshot(snapshot: proto::Snapshot) -> (Snapshot, usize) {
    let mut unreadable = 0;
    let tabs = snapshot
        .tabs
        .into_iter()
        .filter_map(|record| {
            let read = tab(record);
            unreadable += usize::from(read.is_none());
            read
        })
        .collect();
    let converted = Snapshot {
        seq: snapshot.seq,
        instance: snapshot.instance,
        tabs,
        panes: snapshot.panes.into_iter().map(pane).collect(),
        restoring: snapshot.restoring,
    };
    (converted, unreadable)
}

/// What an event means to the mirror. None for one the mirror holds nothing about: a setting
/// changed, which this app sent; an effect the window does not act on; and a daemon handing
/// over, whose connection ends right after, which is what the follower acts on.
pub fn event(event: proto::Event) -> Option<BackendEvent> {
    Some(match event.event? {
        event::Event::PaneOpened(opened) => BackendEvent::PaneOpened(pane(opened.pane?)),
        event::Event::PaneChanged(changed) => BackendEvent::PaneChanged(pane(changed.pane?)),
        event::Event::PaneClosed(closed) => BackendEvent::PaneClosed(PaneId::new(closed.pane)),
        event::Event::TabOpened(opened) => BackendEvent::TabOpened(tab(opened.tab?)?),
        event::Event::TabChanged(changed) => BackendEvent::TabChanged(tab(changed.tab?)?),
        event::Event::TabClosed(closed) => BackendEvent::TabClosed(TabId::new(closed.tab)),
        event::Event::Restored(restored) => BackendEvent::Restored(lost(restored)),
        event::Event::PasteHeld(held) => {
            BackendEvent::PasteHeld { pane: PaneId::new(held.pane), text: held.text }
        }
        event::Event::PaneEffect(proto::PaneEffect {
            pane,
            effect: Some(pane_effect::Effect::ClipboardWrite(write)),
        }) if write.clipboard() == proto::ClipboardKind::Standard => {
            BackendEvent::ClipboardWrite { pane: PaneId::new(pane), text: write.text }
        }
        // The selection and primary clipboards are X11's, and a Mac has neither; the bell,
        // notifications and progress are not carried yet.
        event::Event::SettingsChanged(_)
        | event::Event::PaneEffect(_)
        | event::Event::Replaced(_) => return None,
    })
}

fn lost(restored: proto::Restored) -> Restored {
    Restored {
        lost_tabs: restored.lost_tabs.into_iter().map(TabId::new).collect(),
        lost_panes: restored.lost_panes.into_iter().map(PaneId::new).collect(),
        saving_stopped: restored.saving_stopped,
    }
}

pub fn pane(record: proto::Pane) -> Pane {
    let agent_state = record.agent_state();
    let facts = record.facts.unwrap_or_default();
    Pane {
        id: PaneId::new(record.pane),
        // Which tab holds it is the tab trees' to say; the mirror sets it.
        tab: TabId::new(""),
        agent_state: match agent_state {
            proto::AgentState::Working => AgentState::Working,
            proto::AgentState::Blocked => AgentState::Blocked,
            proto::AgentState::Idle => AgentState::Idle,
            proto::AgentState::Unknown => AgentState::Unknown,
        },
        finished_unseen: record.finished_unseen,
        agent: record.agent,
        cwd: record.cwd,
        name: record.label,
        title: Some(record.title).filter(|title| !title.is_empty()),
        command: record.command,
        facts: AgentFacts {
            context_used: facts.context_used,
            subagents: facts.subagents,
            model: facts.model,
            cost_usd: facts.cost_usd,
            other: facts.other.into_iter().collect(),
        },
    }
}

/// A tab, or none when its tree does not read, which a daemon of this protocol never sends.
pub fn tab(record: proto::Tab) -> Option<Tab> {
    let label = record.label.unwrap_or_default();
    Some(Tab {
        id: TabId::new(record.tab),
        label: label.text.filter(|text| !text.is_empty()),
        generation: label.generation,
        root: node(record.root?)?,
        zoomed: record.zoomed.map(PaneId::new),
    })
}

fn node(given: proto::Node) -> Option<LayoutNode> {
    Some(match given.node? {
        proto::node::Node::Pane(pane) => LayoutNode::Pane(PaneId::new(pane)),
        proto::node::Node::Split(split) => {
            let axis = match split.axis() {
                proto::Axis::Columns => SplitAxis::Columns,
                proto::Axis::Rows => SplitAxis::Rows,
                proto::Axis::Unspecified => return None,
            };
            LayoutNode::Split {
                axis,
                ratio: split.ratio,
                first: Box::new(node(*split.first?)?),
                second: Box::new(node(*split.second?)?),
            }
        }
    })
}

pub fn side(side: Side) -> proto::Side {
    match side {
        Side::Left => proto::Side::Left,
        Side::Right => proto::Side::Right,
        Side::Up => proto::Side::Up,
        Side::Down => proto::Side::Down,
    }
}

pub fn branch(branch: Branch) -> proto::Branch {
    match branch {
        Branch::First => proto::Branch::First,
        Branch::Second => proto::Branch::Second,
    }
}

fn key_event(
    key: muster_core::KeyEvent,
    option_as_alt: OptionAsAlt,
    key_code: fn(muster_core::Key) -> u32,
) -> input_event::Key {
    input_event::Key {
        action: match key.action {
            KeyAction::Press => proto::KeyAction::Press,
            KeyAction::Release => proto::KeyAction::Release,
            KeyAction::Repeated => proto::KeyAction::Repeat,
        }
        .into(),
        key: key_code(key.key),
        mods: u32::from(key.modifiers.0),
        consumed_mods: u32::from(key.consumed_modifiers.0),
        text: key.text,
        unshifted_codepoint: key.unshifted_codepoint.map_or(0, u32::from),
        composing: key.is_composing,
        option_as_alt: match option_as_alt {
            OptionAsAlt::Never => proto::OptionAsAlt::Never,
            OptionAsAlt::Always => proto::OptionAsAlt::Always,
            OptionAsAlt::LeftOnly => proto::OptionAsAlt::Left,
            OptionAsAlt::RightOnly => proto::OptionAsAlt::Right,
        }
        .into(),
    }
}

/// An input event for one pane. `key_code` is libghostty's code for a key, which the caller
/// supplies from the library that has it.
pub fn input(
    pane: &PaneId,
    event: InputEvent,
    key_code: fn(muster_core::Key) -> u32,
) -> proto::InputEvent {
    let input = match event {
        InputEvent::Key { key, option_as_alt } => {
            input_event::Input::Key(key_event(key, option_as_alt, key_code))
        }
        InputEvent::ClearScreen { key, option_as_alt } => {
            input_event::Input::Perform(input_event::Perform {
                action: Some(input_event::perform::Action::ClearScreen(
                    input_event::perform::ClearScreen {},
                )),
                key: key.map(|key| key_event(key, option_as_alt, key_code)),
            })
        }
        InputEvent::Reset => input_event::Input::Perform(input_event::Perform {
            action: Some(input_event::perform::Action::Reset(input_event::perform::Reset {})),
            key: None,
        }),
        InputEvent::Paste { text, confirmed } => {
            input_event::Input::Paste(input_event::Paste { text, confirmed })
        }
        InputEvent::Send { text, enter } => {
            input_event::Input::Send(input_event::Send { text, enter })
        }
        InputEvent::Bytes(bytes) => input_event::Input::Perform(input_event::Perform {
            action: Some(input_event::perform::Action::Raw(bytes)),
            key: None,
        }),
        InputEvent::Wheel(wheel) => input_event::Input::Wheel(input_event::Wheel {
            dx: wheel.dx,
            dy: wheel.dy,
            precise: wheel.precise,
            momentum: u32::from(wheel.momentum),
            mods: u32::from(wheel.modifiers.0),
            x: wheel.x,
            y: wheel.y,
        }),
        InputEvent::Focus(focused) => input_event::Input::Focus(input_event::Focus { focused }),
        InputEvent::Mouse(mouse) => input_event::Input::Mouse(input_event::Mouse {
            action: match mouse.action {
                MouseAction::Press => proto::MouseAction::Press,
                MouseAction::Release => proto::MouseAction::Release,
                MouseAction::Motion => proto::MouseAction::Motion,
            }
            .into(),
            button: match mouse.button {
                MouseButton::None => proto::MouseButton::None,
                MouseButton::Left => proto::MouseButton::Left,
                MouseButton::Right => proto::MouseButton::Right,
                MouseButton::Middle => proto::MouseButton::Middle,
            }
            .into(),
            mods: u32::from(mouse.modifiers.0),
            x: mouse.x,
            y: mouse.y,
        }),
    };
    proto::InputEvent { pane: pane.to_string(), input: Some(input) }
}

pub fn shell(settings: &DaemonSettings) -> proto::Shell {
    let mut shell = proto::Shell {
        command: settings.shell.command.clone(),
        ssh_env: settings.shell.ssh_env,
        ssh_terminfo: settings.shell.ssh_terminfo,
        sudo: settings.shell.sudo,
        ..proto::Shell::default()
    };
    shell.set_mode(match settings.shell.mode {
        ShellMode::Auto => proto::ShellMode::Default,
        ShellMode::Login => proto::ShellMode::Login,
        ShellMode::NonLogin => proto::ShellMode::NonLogin,
    });
    shell
}

pub fn palette(palette: &Palette) -> proto::Palette {
    let mut converted = proto::Palette {
        entries: palette.entries.iter().copied().map(rgb).collect(),
        foreground: rgb(palette.foreground),
        background: rgb(palette.background),
        cursor: palette.cursor.map(rgb),
        ..proto::Palette::default()
    };
    converted.set_scheme(if palette.dark {
        proto::ColorScheme::Dark
    } else {
        proto::ColorScheme::Light
    });
    converted
}

pub fn cursor(settings: &DaemonSettings) -> proto::Cursor {
    let mut cursor = proto::Cursor { blink: settings.cursor.blink, ..proto::Cursor::default() };
    cursor.set_style(match settings.cursor.style {
        None => proto::CursorStyle::Unspecified,
        Some(CursorStyle::Block) => proto::CursorStyle::Block,
        Some(CursorStyle::Bar) => proto::CursorStyle::Bar,
        Some(CursorStyle::Underline) => proto::CursorStyle::Underline,
        Some(CursorStyle::Hollow) => proto::CursorStyle::Hollow,
    });
    cursor
}

fn rgb(color: Rgb) -> u32 {
    u32::from_be_bytes([0, color.red, color.green, color.blue])
}
