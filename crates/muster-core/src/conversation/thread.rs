//! The thread a window shows: rows built from the shared log for that window's view of it.
//!
//! Built so the human finds what was for them without reading the rest (MIP-9, section 1).
//! Messages that woke nobody but agents fold into one line when two or more run together;
//! joins, leaves and changes to the group merge into one row; a line marks where the unread
//! messages began when the conversation opened; and a date divider starts each day.
//!
//! The whole thread is rebuilt when anything in it changes. It is a few hundred rows of
//! already-parsed blocks, and messages arrive at the speed people and agents write.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::PaneKey;
use crate::conversation::log::{Change, Entry, What};
use crate::conversation::{Group, Message};
use crate::diagnostics::clock;
use crate::markdown::{self, Block};
use crate::posting;

/// How many lines of a message the thread shows before "Show all". A body runs to 1 MiB.
pub const LINES: usize = 40;

/// How one window shows the thread: which folds and long messages it opened, and where the
/// unread messages began.
#[derive(Debug, Clone, Default)]
pub struct ThreadView {
    /// The first message that waited for the human when the conversation opened.
    pub unread_from: Option<u64>,
    /// The folds opened, each by its first message's number.
    pub open_folds: BTreeSet<u64>,
    /// The long messages shown whole.
    pub whole: BTreeSet<u64>,
    /// The window's offset from UTC, in seconds, for the dates on the dividers.
    pub utc_offset_s: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// The day the rows after it are from, yyyy-mm-dd.
    Divider {
        date: String,
    },
    /// Consecutive joins, leaves and changes to the group, in words.
    Members {
        text: String,
        at_ms: u64,
    },
    /// A run of messages that woke nobody but agents, opened by choosing it.
    Folded {
        first: u64,
        count: usize,
        from_ms: u64,
        to_ms: u64,
    },
    Message(Box<MessageRow>),
    /// Where the unread messages began.
    Unread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent facts about a message, one each")]
pub struct MessageRow {
    pub seq: u64,
    pub at_ms: u64,
    /// The author as the conversation names them.
    pub author: String,
    /// The author's participant name, which a reply is addressed to.
    pub author_name: String,
    /// The author's pane, for Go to pane, when the author is an agent in one.
    pub author_pane: Option<PaneKey>,
    /// The recipients as the conversation names them.
    pub to: Vec<String>,
    pub to_you: bool,
    pub by_you: bool,
    pub urgent: bool,
    pub blocks: Vec<Block>,
    /// Whether the body goes on past what is shown.
    pub more: bool,
    /// Whether Reply addresses its author: not Muster, not the human, and still a member.
    pub replyable: bool,
    pub from_muster: bool,
    /// The answers it offers, which make it a question (MIP-9, section 6).
    pub answers: Vec<String>,
    /// Who answered it and with what, once somebody has.
    pub answered: Option<Answered>,
    /// Whether the human may pick one of its answers now: asked of them, and not yet answered.
    pub answerable: bool,
    /// The question it answers, when it is a reply that picked one.
    pub answering: Option<u64>,
}

/// Who answered a question, and with which of its answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// As the conversation names them.
    pub by: String,
    pub by_name: String,
    pub answer: String,
}

/// The rows of `group`'s thread as a window with this view of it shows them.
pub fn rows(group: &Group, view: &ThreadView) -> Vec<Row> {
    let answered = group.answers_given();
    let made = Made { group, view, answered: &answered };
    let mut rows = Vec::new();
    let mut day = None;
    let mut run: Vec<&Entry> = Vec::new();
    let mut changes: Vec<&Entry> = Vec::new();
    let mut unread_marked = false;
    for entry in group.log.entries() {
        let date = date(entry.at_ms, view.utc_offset_s);
        let new_day = day.as_ref() != Some(&date);
        let unread_here =
            !unread_marked && view.unread_from.is_some_and(|first| entry.seq >= first);
        let foldable = matches!(&entry.what, What::Message(message) if folds(group, message));
        let is_change = !matches!(entry.what, What::Message(_));
        if new_day || unread_here || !foldable {
            flush_run(&made, &mut run, &mut rows);
        }
        if new_day || unread_here || !is_change {
            flush_changes(group, &mut changes, &mut rows);
        }
        if new_day {
            rows.push(Row::Divider { date: date.clone() });
            day = Some(date);
        }
        if unread_here {
            rows.push(Row::Unread);
            unread_marked = true;
        }
        match &entry.what {
            What::Message(_) if foldable => run.push(entry),
            What::Message(message) => rows.push(message_row(&made, entry, message)),
            _ => changes.push(entry),
        }
    }
    flush_run(&made, &mut run, &mut rows);
    flush_changes(group, &mut changes, &mut rows);
    rows
}

/// What every row of one thread is made from.
struct Made<'a> {
    group: &'a Group,
    view: &'a ThreadView,
    /// Each question answered, by its number: by whom and with what.
    answered: &'a BTreeMap<u64, (&'a str, &'a str)>,
}

/// Whether a message is one that woke nobody but agents.
fn folds(group: &Group, message: &Message) -> bool {
    !posting::is_human(&message.author)
        && !posting::is_muster(&message.author)
        && !group.woke_human(message)
}

fn date(at_ms: u64, utc_offset_s: i32) -> String {
    let at = i64::try_from(at_ms).unwrap_or(i64::MAX);
    clock::format_date(at.saturating_add(i64::from(utc_offset_s) * 1000))
}

fn flush_run(made: &Made, run: &mut Vec<&Entry>, rows: &mut Vec<Row>) {
    let (Some(first), Some(last)) = (run.first(), run.last()) else { return };
    if run.len() >= 2 && !made.view.open_folds.contains(&first.seq) {
        rows.push(Row::Folded {
            first: first.seq,
            count: run.len(),
            from_ms: first.at_ms,
            to_ms: last.at_ms,
        });
    } else {
        for entry in run.iter() {
            if let What::Message(message) = &entry.what {
                rows.push(message_row(made, entry, message));
            }
        }
    }
    run.clear();
}

fn message_row(made: &Made, entry: &Entry, message: &Message) -> Row {
    let group = made.group;
    let (blocks, more) = if made.view.whole.contains(&entry.seq) {
        (message.blocks.clone(), false)
    } else {
        markdown::leading(&message.blocks, LINES)
    };
    let author_pane = group
        .members
        .iter()
        .flatten()
        .find(|member| member.name == message.author)
        .and_then(|member| member.pane.clone());
    let answered = made.answered.get(&entry.seq).map(|(by, answer)| Answered {
        by: group.shown(by),
        by_name: (*by).to_string(),
        answer: (*answer).to_string(),
    });
    Row::Message(Box::new(MessageRow {
        seq: entry.seq,
        at_ms: entry.at_ms,
        author: group.shown(&message.author),
        author_name: message.author.clone(),
        author_pane,
        to: message.to.iter().map(|to| group.shown(to)).collect(),
        to_you: message.to.iter().any(|to| posting::is_human(to)),
        by_you: posting::is_human(&message.author),
        urgent: message.urgent,
        blocks,
        more,
        replyable: group.addressable(&message.author),
        from_muster: posting::is_muster(&message.author),
        answers: message.answers.clone(),
        answerable: answered.is_none() && group.asks_human(message),
        answered,
        answering: message.answering.as_ref().map(|answering| answering.question),
    }))
}

/// Consecutive changes to the group as one row: "otter made the group; heron, lynx and you
/// joined". Each kind is said once, in the order it first appears, naming everybody it covers.
fn flush_changes(group: &Group, changes: &mut Vec<&Entry>, rows: &mut Vec<Row>) {
    let Some(first) = changes.first() else { return };
    let at_ms = first.at_ms;
    let mut said: Vec<(&'static str, Vec<String>)> = Vec::new();
    for entry in changes.iter() {
        let (verb, name) = match &entry.what {
            What::Created(name) => ("made the group", name),
            What::Joined(name) => ("joined", name),
            What::Left(name) => ("left", name),
            What::Changed { by, change: Change::Policy } => ("set the policy", by),
            What::Changed { by, change: Change::Paused } => ("paused it", by),
            What::Changed { by, change: Change::Resumed } => ("resumed it", by),
            What::Message(_) => continue,
        };
        let shown = group.shown(name);
        match said.iter_mut().find(|(said, _)| *said == verb) {
            Some((_, names)) if names.contains(&shown) => {}
            Some((_, names)) => names.push(shown),
            None => said.push((verb, vec![shown])),
        }
    }
    let text = said
        .iter()
        .map(|(verb, names)| format!("{} {verb}", listed(names)))
        .collect::<Vec<_>>()
        .join("; ");
    rows.push(Row::Members { text, at_ms });
    changes.clear();
}

/// "a", "a and b", "a, b and c".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
