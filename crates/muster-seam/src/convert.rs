//! Wire vocabulary to domain vocabulary.
//!
//! Every name arriving here came from a table the shell generated from the same libghostty
//! pin this core encodes with, so a name the core does not know means those two have come
//! apart. That is worth a refusal naming the word: the alternative is a keyboard where a
//! few keys quietly do nothing, which is the failure mode this whole vocabulary exists to
//! avoid.

use std::collections::BTreeMap;

use muster_core::composition::{DaemonId, View, ViewNode};
use muster_core::config::Rgb;
use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers};
use muster_core::mirror::backend::{AgentFacts, SplitAxis};
use muster_core::roster::{Numbering, Roster, machine_color};

use crate::proto;
use crate::session::{DaemonHealth, PaneAgent};

/// What one pane's agent is doing, for the shell, a read, and a watch alike.
pub(crate) fn pane_state(agent: &PaneAgent) -> proto::PaneStateChanged {
    proto::PaneStateChanged {
        daemon_id: agent.pane.daemon.to_string(),
        pane_id: agent.pane.pane.to_string(),
        state: agent.state.as_str().to_string(),
        since_ms: agent.since_ms,
        reported: agent.reported,
        unreadable: agent.unreadable,
        facts: (agent.facts != AgentFacts::default()).then(|| facts(&agent.facts)),
        progress: agent.progress.map(|progress| proto::PaneProgress {
            state: progress.state.as_str().to_string(),
            percent: progress.percent.map(u32::from),
        }),
        rang: agent.rang,
    }
}

fn facts(facts: &AgentFacts) -> proto::AgentFacts {
    proto::AgentFacts {
        context_used: facts.context_used,
        subagents: facts.subagents,
        model: facts.model.clone().unwrap_or_default(),
        cost_usd: facts.cost_usd,
        waiting: facts.waiting.clone().unwrap_or_default(),
        other: facts.other.clone().into_iter().collect(),
    }
}

/// How much of one daemon's truth the window has, for the shell and a watch alike.
pub(crate) fn backend_health(heard: &DaemonHealth) -> proto::BackendHealth {
    proto::BackendHealth {
        daemon_id: heard.daemon.to_string(),
        state: heard.health.as_str().to_string(),
        detail: heard.detail.clone(),
    }
}

/// What the window is showing, on its way out to the shell.
///
/// Ids and paths become strings and absence becomes the empty string, which is proto3's own
/// spelling for a field nobody set. The one place that is not good enough is the tree: a tab
/// whose arrangement has not arrived is a different answer from a tab with no panes, so
/// `root` is an absent message rather than an empty one.
pub(crate) fn view(view: &View) -> proto::ViewChanged {
    proto::ViewChanged {
        tab_id: view.tab.as_ref().map(ToString::to_string).unwrap_or_default(),
        regions: view
            .regions
            .iter()
            .map(|region| proto::ViewRegion {
                region_id: region.id.to_string(),
                daemon_id: region.daemon.to_string(),
                tab_id: region.tab.to_string(),
                pane_id: region.pane.as_ref().map(ToString::to_string).unwrap_or_default(),
                weight: region.weight,
                root: region.root.as_ref().map(node),
                zoomed: region.zoomed,
                daemon_socket: region.daemon_socket.clone().unwrap_or_default(),
                remote: region.remote,
            })
            .collect(),
        focused_region: view.focused.map(|id| id.to_string()).unwrap_or_default(),
    }
}

/// What exists, on its way out to the shell.
///
/// The chords travel as presses on each row rather than as a mode beside the list, so that
/// "what reaches this row" has exactly one answer in the message and a shell cannot combine a
/// mode and a place into a different one than the core did.
///
/// [`counting`] rides alongside and does not weaken that: it says what kind of thing a press
/// names, never what reaches what. Three readers need it and no row can answer them - see
/// `RosterChanged.Counting` in the schema.
pub(crate) fn roster(
    roster: &Roster,
    numbering: &Numbering,
    chosen: &BTreeMap<DaemonId, Rgb>,
) -> proto::RosterChanged {
    proto::RosterChanged {
        counting: counting(numbering).into(),
        machines: roster
            .machines
            .iter()
            .map(|machine| proto::RosterMachine {
                daemon_id: machine.id.to_string(),
                state: machine.health.as_str().to_string(),
                panes: u32::try_from(machine.panes).unwrap_or(u32::MAX),
                color: machine_color(&machine.id, chosen).to_string(),
            })
            .collect(),
        tabs: roster
            .tabs
            .iter()
            .map(|tab| proto::RosterTab {
                tab_id: tab.id.to_string(),
                daemon_ids: tab.daemons.iter().map(ToString::to_string).collect(),
                // Zero is proto3's own spelling for a field nobody set, and the handler
                // already reads it as no place at all - so a number too large to send
                // arrives as unnameable rather than as a different tab. No window holds
                // four billion tabs; this is a floor, not a case anybody meets. Same for a
                // pane's place, below.
                place: u32::try_from(tab.place).unwrap_or_default(),
                tab_press: pressed(numbering.chord_on_tab(tab).tab),
                armed: numbering.armed_on(tab),
                label: tab.label.clone(),
                on_screen: tab.on_screen,
                // Empty is how a string field says nothing was named, the same spelling the
                // appearance vocabulary uses. An optional carrying a name nobody could have
                // typed - the empty one - is not a state worth a wire representation.
                given_name: tab.given_name.clone().unwrap_or_default(),
                panes: tab
                    .panes
                    .iter()
                    .map(|pane| {
                        let chord = numbering.chord_on_pane(tab, pane);
                        proto::RosterPane {
                            daemon_id: pane.key.daemon.to_string(),
                            pane_id: pane.key.pane.to_string(),
                            place: u32::try_from(pane.place).unwrap_or_default(),
                            tab_press: pressed(chord.tab),
                            pane_press: pressed(chord.pane),
                            label: pane.label.clone(),
                            on_screen: pane.on_screen,
                            subtitle: pane.subtitle.clone().unwrap_or_default(),
                            given_name: pane.given_name.clone().unwrap_or_default(),
                        }
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// What the chords are counting, as the wire says it.
///
/// One arm per [`Numbering`] variant and no default, so a fourth variant cannot reach the shell
/// spelled as one of these - which would be a window drawing badges over its panes for a
/// gesture nobody made.
fn counting(numbering: &Numbering) -> proto::roster_changed::Counting {
    match numbering {
        Numbering::Panes => proto::roster_changed::Counting::Panes,
        Numbering::Tabs => proto::roster_changed::Counting::Tabs,
        Numbering::PanesIn(_) => proto::roster_changed::Counting::PanesInTab,
    }
}

/// One press of a chord, as the wire spells "there is none".
///
/// Zero, which is proto3's own word for a field nobody set - and there is no ⌘0 among the
/// numbered chords, so the value cannot be mistaken for a real one.
///
/// Which places count as no press at all is [`muster_core::roster::Chord`]'s answer rather than
/// this one's. It has to be: whether the *rest* of a chord survives depends on it, and a pane
/// past the ninth in its tab has to arrive carrying nothing rather than carrying a tab press
/// that lands somewhere else.
fn pressed(place: Option<usize>) -> u32 {
    place.and_then(|place| u32::try_from(place).ok()).unwrap_or_default()
}

fn node(node: &ViewNode) -> proto::ViewNode {
    let payload = match node {
        ViewNode::Pane(pane) => proto::view_node::Node::Pane(proto::ViewPane {
            pane_id: pane.id.to_string(),
            link_socket_path: pane.link_socket_path.clone().unwrap_or_default(),
            font_size_offset: pane.font_size_offset,
            bridge_restarts: pane.bridge_restarts,
        }),
        ViewNode::Split { axis, ratio, first, second } => {
            proto::view_node::Node::Split(Box::new(proto::ViewSplit {
                axis: match axis {
                    SplitAxis::Columns => "columns".to_string(),
                    SplitAxis::Rows => "rows".to_string(),
                },
                ratio: *ratio,
                first: Some(Box::new(self::node(first))),
                second: Some(Box::new(self::node(second))),
            }))
        }
    };
    proto::ViewNode { node: Some(payload) }
}

pub(crate) fn key(event: &proto::KeyEvent) -> Result<KeyEvent, String> {
    let action = KeyAction::parse(&event.action).ok_or_else(|| {
        format!(
            "the core does not know a key action called {:?}, so that keystroke reached the \
             pane as nothing. Only press, release and repeated exist.",
            event.action
        )
    })?;

    let key = Key::parse(&event.key).ok_or_else(|| {
        format!(
            "the core does not know a key called {:?}, so it reached the pane as nothing - \
             that key will appear dead while every other key works. Both sides' key tables \
             come from tools/gen-keycodes against deps/ghostty.pin, so this means one of \
             them was regenerated without the other.",
            event.key
        )
    })?;

    let modifiers = Modifiers::parse(&event.modifiers).ok_or_else(|| {
        format!(
            "the core does not know one of the modifiers {:?}, so that keystroke reached the \
             pane as nothing rather than as the wrong chord.",
            event.modifiers
        )
    })?;

    let consumed_modifiers = Modifiers::parse(&event.consumed_modifiers).ok_or_else(|| {
        format!(
            "the core does not know one of the consumed modifiers {:?}. Reporting a modifier \
             the layout already spent would send an escape sequence where the user typed a \
             character, so the keystroke was dropped instead.",
            event.consumed_modifiers
        )
    })?;

    Ok(KeyEvent {
        action,
        key,
        modifiers,
        consumed_modifiers,
        text: event.text.clone(),
        text_without_option: event.text_without_option.clone(),
        // A codepoint that is not a character is not worth refusing over: it is an extra the
        // kitty protocol reports, not the keystroke itself.
        unshifted_codepoint: event.unshifted_codepoint.and_then(char::from_u32),
        is_composing: event.is_composing,
    })
}
