//! Turns a herdr subscription's newline-delimited JSON into Muster's vocabulary.
//!
//! A push decoder rather than something owning a reader, matching `FrameDecoder`: one
//! idiom for the two streaming parsers in this crate, and the caller keeps its socket so
//! it can shut the connection down from another thread. Feeding it a recorded transcript
//! with a cut in an awkward place is the whole test strategy, and needs no daemon.
//!
//! This is the only file that knows herdr spells things `pane_created`. Above it, the
//! mirror sees `PaneUpserted` and could be fed by any backend.

use std::collections::BTreeSet;

use muster_core::AgentState;
use muster_core::mirror::BackendEvent;
use muster_core::mirror::backend::{Pane, PaneId, Tab, TabId, Workspace, WorkspaceId};
use serde_json::Value;

use crate::layout::read_layout;

/// Pure, and deliberately so.
///
/// Holds two things across calls: the tail of a line that has not finished arriving, and
/// the set of event names it has already reported as unrecognized.
#[derive(Debug, Default)]
pub struct EventDecoder {
    pending: Vec<u8>,
    /// Every unknown name ever seen, so each is reported once. herdr defines 29 event
    /// kinds and Muster reads 13; the rest arrive on a subscription whether or not anyone
    /// wants them, and a log line per pane per second is how a run log stops being read.
    unknown_seen: BTreeSet<String>,
    unknown_pending: Vec<String>,
}

impl EventDecoder {
    pub fn new() -> EventDecoder {
        EventDecoder::default()
    }

    /// Feeds a chunk of stream and returns the events that completed inside it.
    ///
    /// Anything past the last newline is held. A line routinely arrives in two reads, and
    /// half a JSON object decodes to nothing rather than to something wrong.
    pub fn consume(&mut self, chunk: &[u8]) -> Vec<BackendEvent> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline).take(newline).collect();
            match decode(&line) {
                Decoded::Event(event) => events.push(event),
                Decoded::Unknown(kind) => {
                    if self.unknown_seen.insert(kind.clone()) {
                        self.unknown_pending.push(kind);
                    }
                }
                Decoded::Ignored | Decoded::Unreadable => {}
            }
        }

        events
    }

    /// Event names dropped for the first time since the last call.
    ///
    /// Returned as data rather than logged here, so that the decoder stays pure and the
    /// connection that knows which daemon this is writes the line.
    pub fn take_unknown_kinds(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unknown_pending)
    }
}

enum Decoded {
    Event(BackendEvent),
    /// A name Muster has decided not to read. Silent.
    Ignored,
    /// A well-formed envelope naming something Muster has never seen, or a name it does
    /// read whose payload no longer fits. Both mean go and look.
    Unknown(String),
    /// Not JSON, or JSON that is not an envelope. Distinguished from `Unknown` because
    /// there is no name to report and nothing a reader could act on.
    Unreadable,
}

/// Decodes one line.
///
/// Keyed on the envelope's `event` rather than on `data.type`, which usually agrees with
/// it. Usually: `pane.agent_status_changed` has no `type` inside its data at all, because
/// the parameterized subscriptions answer with a different schema than the session-wide
/// ones (`corpus/herdr-0.8.0/api-schema.json`, `subscription_event` versus `event`).
/// `event` is required by both.
fn decode(line: &[u8]) -> Decoded {
    let Ok(envelope) = serde_json::from_slice::<Value>(line) else { return Decoded::Unreadable };
    let Some(kind) = envelope.get("event").and_then(Value::as_str) else {
        return Decoded::Unreadable;
    };
    let data = envelope.get("data").unwrap_or(&Value::Null);

    let event = match kind {
        "workspace_created" | "workspace_updated" | "workspace_renamed" => {
            data.get("workspace").and_then(read_workspace).map(BackendEvent::WorkspaceUpserted)
        }
        // Carries the whole workspace rather than its id, unlike every other closure.
        // Reading either spelling means a client is not broken by which one herdr sends.
        "workspace_closed" => id(data, "workspace_id")
            .or_else(|| data.get("workspace").and_then(|w| id(w, "workspace_id")))
            .map(|id| BackendEvent::WorkspaceRemoved(WorkspaceId::new(id))),
        "tab_created" | "tab_renamed" => {
            data.get("tab").and_then(read_tab).map(BackendEvent::TabUpserted)
        }
        "tab_closed" => id(data, "tab_id").map(|id| BackendEvent::TabRemoved(TabId::new(id))),
        "pane_created" | "pane_updated" => {
            data.get("pane").and_then(read_pane).map(BackendEvent::PaneUpserted)
        }
        // Two names for one outcome. A pane whose program ended emits `pane_exited` and
        // never a `pane_closed` afterwards, so a mirror keyed on the latter alone renders
        // dead panes forever (`observations/herdr-0.8.0.md` section 10).
        "pane_closed" | "pane_exited" => {
            id(data, "pane_id").map(|id| BackendEvent::PaneRemoved(PaneId::new(id)))
        }
        // Both spellings, because they are the same fact from two schemas: the dotted one
        // is what a per-pane subscription answers with, the snake one is in the
        // session-wide enum. Only the dotted one has ever been observed arriving
        // (`observations/herdr-0.8.0.md` section 11), and reading only what we have seen
        // would make a daemon that starts sending the other look like an agent that never
        // moves.
        "pane.agent_status_changed" | "pane_agent_status_changed" => {
            id(data, "pane_id").map(|pane| BackendEvent::AgentStateChanged {
                pane: PaneId::new(pane),
                state: AgentState::from_backend(text(data, "agent_status")),
            })
        }
        "pane_agent_detected" => id(data, "pane_id").and_then(|pane| {
            data.get("agent").and_then(Value::as_str).map(|agent| BackendEvent::AgentDetected {
                pane: PaneId::new(pane),
                agent: agent.to_string(),
            })
        }),
        // Each focus event carries every cursor it knows, and the mirror treats absence as
        // "says nothing" rather than as a clear. So naming the workspace on all three is
        // free and leaves the workspace cursor right even if its own event went missing.
        "workspace_focused" => Some(BackendEvent::FocusMoved {
            workspace: id(data, "workspace_id").map(WorkspaceId::new),
            tab: None,
            pane: None,
        }),
        "tab_focused" => Some(BackendEvent::FocusMoved {
            workspace: id(data, "workspace_id").map(WorkspaceId::new),
            tab: id(data, "tab_id").map(TabId::new),
            pane: None,
        }),
        "pane_focused" => Some(BackendEvent::FocusMoved {
            workspace: id(data, "workspace_id").map(WorkspaceId::new),
            tab: None,
            pane: id(data, "pane_id").map(PaneId::new),
        }),
        // The whole tab, in absolute values, so applying it twice is applying it once. It
        // follows every pane change and no tab or workspace change, which is why the mirror
        // cascades a tab's tree itself rather than waiting to be told
        // (`observations/herdr-0.8.0.md` sections 10 and 13).
        "layout_updated" => {
            data.get("layout").and_then(read_layout).map(BackendEvent::LayoutUpserted)
        }
        // Recognized, and deliberately not read. Kept apart from the unknown set so that
        // set keeps meaning "herdr is sending something we have never seen", which is the
        // drift signal. These describe things the mirror does not model.
        //
        // Deliberately short. `pane_moved` and the reordering events are absent because
        // they plausibly do change what the mirror holds and no recording exists of one -
        // leaving them unknown means the first arrival says so in the run log rather than
        // being silently correct-looking.
        "pane_output_changed"
        | "pane.output_matched"
        | "pane.scroll_changed"
        | "workspace_metadata_updated"
        | "worktree_created"
        | "worktree_opened"
        | "worktree_removed" => return Decoded::Ignored,
        _ => return Decoded::Unknown(kind.to_string()),
    };

    // A known name whose payload will not read is reported as unknown rather than
    // dropped: it means herdr moved a field, which is the drift the run log exists to
    // surface.
    event.map_or_else(|| Decoded::Unknown(kind.to_string()), Decoded::Event)
}

fn read_workspace(value: &Value) -> Option<Workspace> {
    Some(Workspace {
        id: WorkspaceId::new(id(value, "workspace_id")?),
        label: text(value, "label").to_string(),
    })
}

fn read_tab(value: &Value) -> Option<Tab> {
    Some(Tab {
        id: TabId::new(id(value, "tab_id")?),
        workspace: WorkspaceId::new(id(value, "workspace_id")?),
        label: text(value, "label").to_string(),
    })
}

fn read_pane(value: &Value) -> Option<Pane> {
    Some(Pane {
        id: PaneId::new(id(value, "pane_id")?),
        tab: TabId::new(id(value, "tab_id")?),
        workspace: WorkspaceId::new(id(value, "workspace_id")?),
        agent_state: AgentState::from_backend(text(value, "agent_status")),
        // Null rather than absent when there is no agent, and both mean the same here.
        agent: value.get("agent").and_then(Value::as_str).map(str::to_string),
        // Optional in herdr's schema even though every recording carries it. Empty reads
        // as "not stated", which is what a pane whose cwd herdr could not resolve is.
        cwd: text(value, "cwd").to_string(),
        name: optional(value, "label"),
        revision: value.get("revision").and_then(Value::as_u64).unwrap_or_default(),
        // The stripped spelling, so the activity glyph a harness spins in front of its
        // title never reaches a reader. herdr strips it and announces only when the
        // stripped text changes, which is what makes a spinning agent cost nothing
        // (`observations/herdr-0.8.0.md` section 16).
        title: optional(value, "terminal_title_stripped"),
    })
}

/// A required identifier: present, a string, and not empty.
///
/// Empty is refused because an id is a lookup key, and `""` finds nothing in a way that
/// looks like the entity was never created rather than like the wire was wrong.
fn id(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// A field that may be absent, null, or empty, where all three mean "nothing to show".
///
/// Distinct from [`text`], which flattens those to `""`: for a name or a title the
/// difference between "" and nothing is a blank line drawn under a row.
fn optional(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string)
}
