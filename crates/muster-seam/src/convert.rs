//! Wire vocabulary to domain vocabulary.
//!
//! Every name arriving here came from a table the shell generated from the same libghostty
//! pin this core encodes with, so a name the core does not know means those two have come
//! apart. That is worth a refusal naming the word: the alternative is a keyboard where a
//! few keys quietly do nothing, which is the failure mode this whole vocabulary exists to
//! avoid.

use muster_core::composition::{View, ViewNode};
use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers};
use muster_core::mirror::backend::SplitAxis;

use crate::proto;

/// What the window is showing, on its way out to the shell.
///
/// Ids and paths become strings and absence becomes the empty string, which is proto3's own
/// spelling for a field nobody set. The one place that is not good enough is the tree: a tab
/// whose arrangement has not arrived is a different answer from a tab with no panes, so
/// `root` is an absent message rather than an empty one.
pub(crate) fn view(view: &View) -> proto::ViewChanged {
    proto::ViewChanged {
        regions: view
            .regions
            .iter()
            .map(|region| proto::ViewRegion {
                region_id: region.id.to_string(),
                daemon_id: region.daemon.to_string(),
                tab_id: region.tab.to_string(),
                pane_id: region.pane.as_ref().map(ToString::to_string).unwrap_or_default(),
                root: region.root.as_ref().map(node),
                zoomed: region.zoomed,
            })
            .collect(),
        focused_region: view.focused.map(|id| id.to_string()).unwrap_or_default(),
    }
}

fn node(node: &ViewNode) -> proto::ViewNode {
    let payload = match node {
        ViewNode::Pane(pane) => proto::view_node::Node::Pane(proto::ViewPane {
            pane_id: pane.id.to_string(),
            control_socket_path: pane.control_socket_path.clone().unwrap_or_default(),
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
        // A codepoint that is not a character is not worth refusing over: it is an extra the
        // kitty protocol reports, not the keystroke itself.
        unshifted_codepoint: event.unshifted_codepoint.and_then(char::from_u32),
        is_composing: event.is_composing,
    })
}
