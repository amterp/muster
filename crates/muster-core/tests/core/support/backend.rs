//! A daemon's world, as a case spells it.
//!
//! One reader, because more than one corpus describes a session and two readers would let
//! the same JSON mean two things - which is the failure the whole conformance arrangement
//! exists to prevent (`crates/conformance/src/lib.rs`).

use std::fmt::Write as _;

use muster_core::AgentState;
use muster_core::attention::HumanNotice;
use muster_core::composition::{Daemon, Endpoint};
use muster_core::mirror::backend::{
    AgentFacts, CopyNode, LayoutNode, Pane, PaneId, Snapshot, SplitAxis, Tab, TabId,
};
use serde_json::Value;

pub(crate) fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// One attached daemon, as a line.
///
/// Here rather than in either driver because two corpora expect these strings - a config
/// file produces daemons and a composition holds them - and two renderings would let the
/// same line mean two things in two files.
pub(crate) fn describe_daemon(daemon: &Daemon) -> String {
    let mut line = daemon.id.to_string();
    match &daemon.endpoint {
        Endpoint::Local { socket_path } => {
            line.push_str(" local");
            if let Some(path) = socket_path {
                let _ = write!(line, "={path}");
            }
        }
        Endpoint::Ssh { host, options, socket_path } => {
            let _ = write!(line, " ssh={host}");
            if let Some(path) = socket_path {
                let _ = write!(line, " socket={path}");
            }
            if !options.is_empty() {
                let _ = write!(line, " options={}", options.join(" "));
            }
        }
    }
    line
}

pub(crate) fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// A daemon's snapshot: its tabs, each with its tree, and its panes. A pane's tab is whichever
/// tree names it, so a case does not say it twice.
pub(crate) fn read_snapshot(given: &Value) -> Snapshot {
    Snapshot {
        seq: given.get("seq").and_then(Value::as_u64).unwrap_or_default(),
        instance: given.get("instance").and_then(Value::as_u64).unwrap_or(1),
        tabs: collect(given, "tabs", read_tab),
        panes: collect(given, "panes", read_pane),
        restoring: given.get("restoring").and_then(Value::as_bool).unwrap_or(false),
        restored_from_file: given.get("restoredFromFile").and_then(Value::as_bool).unwrap_or(false),
        human: given
            .get("human")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(group, notice)| (group.clone(), read_human_notice(notice)))
            .collect(),
    }
}

/// What waits for the human in one group: `{ "last": 5, "count": 1, "to_you": 1, "from": ["a"] }`,
/// and `"member": true` while the human is in it.
pub(crate) fn read_human_notice(given: &Value) -> HumanNotice {
    let number = |key: &str| given.get(key).and_then(Value::as_u64).unwrap_or_default();
    HumanNotice {
        first: number("first"),
        last: number("last"),
        count: number("count"),
        to_you: number("to_you"),
        from: collect(given, "from", |from| from.as_str().unwrap_or_default().to_string()),
        member: given.get("member").and_then(Value::as_bool).unwrap_or_default(),
        said: String::new(),
    }
}

fn collect<T>(given: &Value, key: &str, read: impl Fn(&Value) -> T) -> Vec<T> {
    given.get(key).and_then(Value::as_array).into_iter().flatten().map(read).collect()
}

/// A tab as a daemon sends it. `copy`, the daemon's whole tree in the text its harness prints
/// (`[a|~x@devenv 0.50]`), is given only for a tab spanning machines, and is `root` otherwise;
/// `copyZoomed` likewise, for a zoom on another machine's pane, and is `zoomed` otherwise.
pub(crate) fn read_tab(given: &Value) -> Tab {
    let root = read_node(given.get("root").unwrap_or(&Value::Null));
    let zoomed = optional(given, "zoomed").map(PaneId::new);
    Tab {
        id: TabId::new(text(given, "id")),
        label: optional(given, "label"),
        generation: given.get("generation").and_then(Value::as_u64).unwrap_or_default(),
        copy: optional(given, "copy").map_or_else(|| CopyNode::of(&root), |copy| read_copy(&copy)),
        root,
        copy_zoomed: optional(given, "copyZoomed").map(PaneId::new).or_else(|| zoomed.clone()),
        zoomed,
        hidden: given.get("hidden").and_then(Value::as_bool).unwrap_or_default(),
        layout_generation: given
            .get("layoutGeneration")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

/// A daemon's whole tree, in the text its harness prints: `[a|b 0.50]` for columns, `[a/b 0.50]`
/// for rows, `~x@devenv` for another machine's pane.
pub(crate) fn read_copy(text: &str) -> CopyNode {
    fn node(text: &str) -> (CopyNode, &str) {
        if let Some(rest) = text.strip_prefix('[') {
            let (first, rest) = node(rest);
            let axis = match rest.chars().next() {
                Some('|') => SplitAxis::Columns,
                Some('/') => SplitAxis::Rows,
                other => panic!("a split in {text:?} has {other:?} where | or / goes"),
            };
            let (second, rest) = node(&rest[1..]);
            let rest = rest.strip_prefix(' ').expect("a ratio follows a space");
            let end = rest.find(']').expect("a split ends with ]");
            let ratio = rest[..end].parse().expect("a ratio is a number");
            let split =
                CopyNode::Split { axis, ratio, first: Box::new(first), second: Box::new(second) };
            (split, &rest[end + 1..])
        } else {
            let end = text.find(['|', '/', ' ', ']']).unwrap_or(text.len());
            let leaf = match text[..end].strip_prefix('~') {
                Some(foreign) => {
                    let (pane, daemon) = foreign.split_once('@').expect("~pane@daemon");
                    CopyNode::Foreign { pane: PaneId::new(pane), daemon: daemon.to_string() }
                }
                None => CopyNode::Own(PaneId::new(&text[..end])),
            };
            (leaf, &text[end..])
        }
    }
    let (read, rest) = node(text);
    assert!(rest.is_empty(), "{rest:?} is left over after the tree in {text:?}");
    read
}

pub(crate) fn read_pane(given: &Value) -> Pane {
    Pane {
        id: PaneId::new(text(given, "id")),
        // Set by the mirror from the trees; a case never has to.
        tab: TabId::new(""),
        agent_state: AgentState::from_backend(&text(given, "agentState")),
        finished_unseen: given.get("finishedUnseen").and_then(Value::as_bool).unwrap_or_default(),
        agent: optional(given, "agent"),
        compactable: given.get("compactable").and_then(Value::as_bool).unwrap_or_default(),
        cwd: text(given, "cwd"),
        name: optional(given, "name"),
        callsign: optional(given, "callsign"),
        former_names: Vec::new(),
        title: optional(given, "title"),
        command: optional(given, "command"),
        facts: given.get("facts").map(read_facts).unwrap_or_default(),
        reported: given.get("reported").and_then(Value::as_bool).unwrap_or_default(),
        unreadable: given.get("unreadable").and_then(Value::as_bool).unwrap_or_default(),
        adapter: muster_core::mirror::Adapter::default(),
        unread_input: given.get("unreadInput").map(|unread| muster_core::mirror::UnreadInput {
            bytes: unread
                .get("bytes")
                .and_then(Value::as_u64)
                .and_then(|bytes| u32::try_from(bytes).ok())
                .unwrap_or_default(),
            since_ms: unread.get("sinceMs").and_then(Value::as_u64).unwrap_or_default(),
            program: text(unread, "program"),
            pid: 0,
        }),
        quiet: given.get("quiet").and_then(Value::as_bool).unwrap_or_default(),
        session: optional(given, "session"),
        resume: given.get("resume").map(|resume| muster_core::mirror::Resume {
            command: collect(resume, "command", |word| word.as_str().unwrap_or_default().into()),
            uncarried: optional(resume, "uncarried"),
        }),
        compaction: match given.get("compaction").and_then(Value::as_str) {
            Some("queued") => muster_core::mirror::PendingCompaction::Queued,
            Some("follow_up") => muster_core::mirror::PendingCompaction::FollowUp,
            _ => muster_core::mirror::PendingCompaction::Nothing,
        },
        maker: optional(given, "maker").map(PaneId::new),
        maker_closed: false,
        state_since_ms: None,
    }
}

fn read_facts(given: &Value) -> AgentFacts {
    AgentFacts {
        #[expect(clippy::cast_possible_truncation, reason = "a percentage, which an f32 holds")]
        context_used: given.get("contextUsed").and_then(Value::as_f64).map(|used| used as f32),
        subagents: given
            .get("subagents")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or_default(),
        model: optional(given, "model"),
        cost_usd: given.get("costUsd").and_then(Value::as_f64),
        other: given
            .get("other")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
        waiting: optional(given, "waiting"),
    }
}

/// A tree written the way a case reads best: a string is a pane, an object is a split.
///
/// The alternative - a `type` discriminant on every node - triples the size of a case for
/// no information, and these cases are meant to be read.
pub(crate) fn read_node(given: &Value) -> LayoutNode {
    if let Some(pane) = given.as_str() {
        return LayoutNode::Pane(PaneId::new(pane));
    }
    let axis = match text(given, "axis").as_str() {
        "columns" => SplitAxis::Columns,
        "rows" => SplitAxis::Rows,
        other => panic!("corpus case names a split axis the driver does not know: {other:?}"),
    };
    LayoutNode::Split {
        axis,
        ratio: ratio(given),
        first: Box::new(read_node(given.get("first").unwrap_or(&Value::Null))),
        second: Box::new(read_node(given.get("second").unwrap_or(&Value::Null))),
    }
}

/// JSON has one number type and a ratio is an `f32`, so narrowing is what reading one is.
///
/// Shared, because two corpora spell a ratio - a pane's split and a region's boundary - and
/// two readers would let the same number mean two things.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn ratio(given: &Value) -> f32 {
    given.get("ratio").and_then(Value::as_f64).unwrap_or(0.5) as f32
}
