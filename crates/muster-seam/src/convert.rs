//! Wire vocabulary to domain vocabulary.
//!
//! Every name arriving here came from a table the shell generated from the same libghostty
//! pin this core encodes with, so a name the core does not know means those two have come
//! apart. That is worth a refusal naming the word: the alternative is a keyboard where a
//! few keys quietly do nothing, which is the failure mode this whole vocabulary exists to
//! avoid.

use std::collections::BTreeMap;

use muster_core::banners::{Banner, Shown, Stack};
use muster_core::composition::{DaemonId, View, ViewPane, ViewTree};
use muster_core::config::Rgb;
use muster_core::conversation::{
    Completion, Draft, Group, Header, Liveness, MemberRow, MessageRow, Row,
};
use muster_core::input::{Key, KeyAction, KeyEvent, Modifiers};
use muster_core::markdown::Block;
use muster_core::mirror::backend::{AgentFacts, SplitAxis};
use muster_core::posting::Sent;
use muster_core::roster::{Numbering, Roster, RosterTab, machine_color};

use crate::proto;
use crate::session::{DaemonHealth, PaneAgent};

/// What one pane's agent is doing, for the shell, a read, and a watch alike.
pub(crate) fn pane_state(agent: &PaneAgent) -> proto::PaneStateChanged {
    proto::PaneStateChanged {
        daemon_id: agent.pane.daemon.to_string(),
        pane_id: agent.pane.pane.to_string(),
        state: agent.state.as_str().to_string(),
        last_state: agent.last_state.map(|state| state.as_str().to_string()).unwrap_or_default(),
        since_ms: agent.since_ms,
        reported: agent.reported,
        unreadable: agent.unreadable,
        facts: (agent.facts != AgentFacts::default()).then(|| facts(&agent.facts)),
        progress: agent.progress.map(|progress| proto::PaneProgress {
            state: progress.state.as_str().to_string(),
            percent: progress.percent.map(u32::from),
        }),
        rang: agent.rang,
        // A watch fills it in; the shell and a read have the roster for it.
        label: String::new(),
        adapter: agent.adapter.as_str().to_string(),
        unread_input: agent.unread_input.as_ref().map(|unread| proto::UnreadInput {
            bytes: unread.bytes,
            since_ms: i64::try_from(unread.since_ms).unwrap_or(i64::MAX),
            program: unread.program.clone(),
            pid: unread.pid,
        }),
        session_id: agent.session.as_ref().map(|session| session.id.clone()).unwrap_or_default(),
        resume: agent.session.as_ref().and_then(|session| session.resume.as_ref()).map(|resume| {
            Box::new(proto::SessionResume {
                command: resume.command.clone(),
                uncarried: resume.uncarried.clone().unwrap_or_default(),
            })
        }),
        compaction: agent.compaction.as_str().to_string(),
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
/// `tree` is an absent message rather than an empty one.
pub(crate) fn view(view: &View) -> proto::ViewChanged {
    proto::ViewChanged {
        tab_id: view.tab.as_ref().map(ToString::to_string).unwrap_or_default(),
        tree: view.tree.as_ref().map(tree),
        zoomed_pane_id: view
            .zoomed
            .as_ref()
            .map(|zoomed| zoomed.pane.to_string())
            .unwrap_or_default(),
        keyboard_pane_id: view
            .keyboard
            .as_ref()
            .map(|keyboard| keyboard.pane.to_string())
            .unwrap_or_default(),
        machines: view
            .machines
            .iter()
            .map(|machine| proto::ViewMachine {
                daemon_id: machine.daemon.to_string(),
                daemon_socket: machine.socket.clone().unwrap_or_default(),
                remote: machine.remote,
            })
            .collect(),
        conversation_daemon_id: view
            .conversation
            .as_ref()
            .map(|group| group.daemon.to_string())
            .unwrap_or_default(),
        conversation_group: view
            .conversation
            .as_ref()
            .map(|group| group.group.clone())
            .unwrap_or_default(),
    }
}

fn tree(tree: &ViewTree) -> proto::ViewTree {
    let node = match tree {
        ViewTree::Pane { daemon, pane } => proto::view_tree::Node::Pane(proto::ViewTreePane {
            daemon_id: daemon.to_string(),
            pane: Some(self::pane(pane)),
        }),
        ViewTree::Split { axis, ratio, first, second } => {
            proto::view_tree::Node::Split(Box::new(proto::ViewTreeSplit {
                axis: self::axis(*axis),
                ratio: *ratio,
                first: Some(Box::new(self::tree(first))),
                second: Some(Box::new(self::tree(second))),
            }))
        }
    };
    proto::ViewTree { node: Some(node) }
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
                restarted: machine.restarted.clone().unwrap_or_default(),
            })
            .collect(),
        tabs: roster.tabs.iter().map(|tab| roster_tab(tab, Some(numbering))).collect(),
        hidden: roster.hidden.iter().map(|tab| roster_tab(tab, None)).collect(),
    }
}

/// One tab and its panes, with the chords `numbering` puts on them. A hidden tab has none, since
/// nothing numbers it.
fn roster_tab(tab: &RosterTab, numbering: Option<&Numbering>) -> proto::RosterTab {
    proto::RosterTab {
        tab_id: tab.id.to_string(),
        daemon_ids: tab.daemons.iter().map(ToString::to_string).collect(),
        // Zero is proto3's own spelling for a field nobody set, and the handler
        // already reads it as no place at all - so a number too large to send
        // arrives as unnameable rather than as a different tab. No window holds
        // four billion tabs; this is a floor, not a case anybody meets. Same for a
        // pane's place, below.
        place: u32::try_from(tab.place).unwrap_or_default(),
        tab_press: numbering.map_or(0, |numbering| pressed(numbering.chord_on_tab(tab).tab)),
        armed: numbering.is_some_and(|numbering| numbering.armed_on(tab)),
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
                let (tab_press, pane_press) = numbering.map_or((0, 0), |numbering| {
                    let chord = numbering.chord_on_pane(tab, pane);
                    (pressed(chord.tab), pressed(chord.pane))
                });
                proto::RosterPane {
                    daemon_id: pane.key.daemon.to_string(),
                    pane_id: pane.key.pane.to_string(),
                    place: u32::try_from(pane.place).unwrap_or_default(),
                    tab_press,
                    pane_press,
                    label: pane.label.clone(),
                    on_screen: pane.on_screen,
                    subtitle: pane.subtitle.clone().unwrap_or_default(),
                    given_name: pane.given_name.clone().unwrap_or_default(),
                    name: pane.name.clone().unwrap_or_default(),
                    callsign: pane.callsign.clone().unwrap_or_default(),
                    beside_callsign: pane.beside_callsign.clone(),
                    compactable: pane.compactable,
                    resumes_as: pane.resumes_as.clone().unwrap_or_default(),
                    quiet: pane.quiet,
                    maker: pane.maker.as_ref().map(ToString::to_string).unwrap_or_default(),
                    under: pane.under.as_ref().map(ToString::to_string).unwrap_or_default(),
                }
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

fn pane(pane: &ViewPane) -> proto::ViewPane {
    proto::ViewPane {
        pane_id: pane.id.to_string(),
        link_socket_path: pane.link_socket_path.clone().unwrap_or_default(),
        font_size_offset: pane.font_size_offset,
        bridge_restarts: pane.bridge_restarts,
    }
}

fn axis(axis: SplitAxis) -> String {
    match axis {
        SplitAxis::Columns => "columns".to_string(),
        SplitAxis::Rows => "rows".to_string(),
    }
}

/// The banners to draw, for the shell.
pub(crate) fn banners(stack: &Stack) -> proto::BannersChanged {
    proto::BannersChanged {
        shown: stack.shown.iter().map(banner).collect(),
        folded: u32::try_from(stack.folded).unwrap_or(u32::MAX),
        oldest_daemon_id: stack.oldest.as_ref().map(|g| g.daemon.to_string()).unwrap_or_default(),
        oldest_group: stack.oldest.as_ref().map(|g| g.group.clone()).unwrap_or_default(),
    }
}

fn banner(banner: &Banner) -> proto::Banner {
    let mut drawn = proto::Banner {
        daemon_id: banner.group.daemon.to_string(),
        group: banner.group.group.clone(),
        seq: banner.shown.seq(),
        replyable: banner.replyable,
        held: banner.held,
        sent: banner.sent.as_ref().map(sent),
        ..proto::Banner::default()
    };
    match &banner.shown {
        Shown::Message(message) => {
            drawn.author = banner.named(&message.author);
            drawn.author_name.clone_from(&message.author);
            drawn.urgent = message.urgent;
            drawn.at_ms = message.at_ms;
            drawn.body = markdown(&message.body);
            drawn.more = message.more;
            drawn.answers.clone_from(&message.answers);
            drawn.picked = banner.picked.and_then(|picked| u32::try_from(picked).ok());
        }
        Shown::Unloaded { from, count, why, .. } => {
            drawn.from = from.iter().map(|name| banner.named(name)).collect();
            drawn.count = *count;
            drawn.unloaded.clone_from(why);
        }
        // Never drawn: the core's stack leaves out a banner still fetching its message.
        Shown::Loading { .. } => {}
    }
    if let Some(waiting) = &banner.waiting {
        drawn.waiting_author = banner.named(&waiting.author);
        drawn.waiting_urgent = waiting.urgent;
    }
    drawn
}

pub(crate) fn sent(sent: &Sent) -> proto::PostOutcome {
    use proto::post_outcome::State;
    let (state, said, heard, body) = match sent {
        Sent::Pending { text } => (State::Pending, "", false, text.as_str()),
        Sent::Posted { said, heard, .. } => (State::Posted, said.as_str(), *heard, ""),
        Sent::Refused { text, why } => (State::Refused, why.as_str(), false, text.as_str()),
        Sent::Unknown { text, why } => (State::Unknown, why.as_str(), false, text.as_str()),
    };
    proto::PostOutcome {
        state: state.into(),
        said: said.to_string(),
        heard,
        body: body.to_string(),
    }
}

/// A message's Markdown, as the core parsed it.
pub(crate) fn markdown(blocks: &[Block]) -> Vec<proto::MarkdownBlock> {
    use proto::markdown_block::Kind;
    blocks
        .iter()
        .map(|block| proto::MarkdownBlock {
            kind: Some(match block {
                Block::Paragraph(runs) => Kind::Paragraph(proto::MarkdownParagraph {
                    runs: runs
                        .iter()
                        .map(|run| proto::MarkdownRun {
                            text: run.text.clone(),
                            emphasis: run.emphasis,
                            strong: run.strong,
                            code: run.code,
                            link: run.link.clone().unwrap_or_default(),
                        })
                        .collect(),
                }),
                Block::List(list) => Kind::List(proto::MarkdownList {
                    numbered: list.start.is_some(),
                    start: list.start.unwrap_or_default(),
                    items: list
                        .items
                        .iter()
                        .map(|item| proto::MarkdownItem { blocks: markdown(item) })
                        .collect(),
                }),
                Block::Code(code) => Kind::Code(proto::MarkdownCode {
                    language: code.language.clone(),
                    text: code.text.clone(),
                }),
            }),
        })
        .collect()
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

/// One window's conversation: the header, the thread's rows and the members column.
pub(crate) fn conversation(
    group: &muster_core::attention::GroupKey,
    header: &Header,
    rows: &[Row],
    posted: Option<&(u64, String)>,
) -> proto::ConversationChanged {
    proto::ConversationChanged {
        daemon_id: group.daemon.to_string(),
        group: group.group.clone(),
        header: Some(proto::ConversationHeader {
            group: header.group.clone(),
            keeper: header.keeper.clone().unwrap_or_default(),
            members: header.members.map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX)),
            members_known: header.members.is_some(),
            paused: header.paused,
            behind: header.behind.clone().unwrap_or_default(),
        }),
        rows: rows.iter().map(|row| conversation_row(row, posted)).collect(),
        ..proto::ConversationChanged::default()
    }
}

/// One window's members column.
pub(crate) fn conversation_members(
    group: &muster_core::attention::GroupKey,
    members: &[MemberRow],
) -> proto::ConversationMembersChanged {
    proto::ConversationMembersChanged {
        daemon_id: group.daemon.to_string(),
        group: group.group.clone(),
        members: members.iter().map(conversation_member).collect(),
    }
}

fn conversation_row(row: &Row, posted: Option<&(u64, String)>) -> proto::ConversationRow {
    use proto::conversation_row::Kind;
    let kind = match row {
        Row::Divider { date } => Kind::Divider(date.clone()),
        Row::Members { text, at_ms } => {
            Kind::Changes(proto::ConversationChanges { text: text.clone(), at_ms: *at_ms })
        }
        Row::Folded { first, count, from_ms, to_ms } => Kind::Fold(proto::ConversationFold {
            first: *first,
            count: u32::try_from(*count).unwrap_or(u32::MAX),
            from_ms: *from_ms,
            to_ms: *to_ms,
        }),
        Row::Message(message) => Kind::Message(conversation_message(message, posted)),
        Row::Unread => Kind::Unread(true),
    };
    proto::ConversationRow { kind: Some(kind) }
}

fn conversation_message(
    message: &MessageRow,
    posted: Option<&(u64, String)>,
) -> proto::ConversationMessage {
    proto::ConversationMessage {
        seq: message.seq,
        at_ms: message.at_ms,
        author: message.author.clone(),
        author_name: message.author_name.clone(),
        author_daemon_id: message
            .author_pane
            .as_ref()
            .map(|pane| pane.daemon.to_string())
            .unwrap_or_default(),
        author_pane_id: message
            .author_pane
            .as_ref()
            .map(|pane| pane.pane.to_string())
            .unwrap_or_default(),
        to: message.to.clone(),
        to_you: message.to_you,
        by_you: message.by_you,
        urgent: message.urgent,
        body: markdown(&message.blocks),
        more: message.more,
        replyable: message.replyable,
        from_muster: message.from_muster,
        said: posted
            .filter(|(seq, _)| *seq == message.seq)
            .map(|(_, said)| said.clone())
            .unwrap_or_default(),
    }
}

fn conversation_member(member: &MemberRow) -> proto::ConversationMember {
    use proto::conversation_member::Liveness as Wire;
    let liveness = match member.liveness {
        Liveness::Alive => Wire::Alive,
        Liveness::Gone => Wire::Gone,
        Liveness::Human => Wire::Human,
        Liveness::Unreachable => Wire::Unreachable,
    };
    proto::ConversationMember {
        name: member.name.clone(),
        shown: member.shown.clone(),
        liveness: liveness.into(),
        daemon_id: member.pane.as_ref().map(|pane| pane.daemon.to_string()).unwrap_or_default(),
        pane_id: member.pane.as_ref().map(|pane| pane.pane.to_string()).unwrap_or_default(),
        unread_first: member.unread.as_ref().map_or(0, |unread| unread.first),
        unread_from: member.unread.as_ref().map(|unread| unread.from.clone()).unwrap_or_default(),
        unread_why: member.unread.as_ref().map(|unread| unread.why.clone()).unwrap_or_default(),
        you: member.you,
    }
}

/// A message box's draft, with where its cursor goes and what the name at it may complete to.
pub(crate) fn message_box_draft(
    draft: &Draft,
    cursor: usize,
    completions: &[Completion],
    group: &Group,
) -> proto::MessageBoxDraft {
    proto::MessageBoxDraft {
        text: draft.text.clone(),
        to: draft
            .to
            .iter()
            .map(|name| proto::Recipient { name: name.clone(), shown: group.shown(name) })
            .collect(),
        urgent: draft.urgent,
        cursor: u32::try_from(cursor).unwrap_or(u32::MAX),
        completions: completions
            .iter()
            .map(|completion| proto::Recipient {
                name: completion.name.clone(),
                shown: completion.shown.clone(),
            })
            .collect(),
    }
}
