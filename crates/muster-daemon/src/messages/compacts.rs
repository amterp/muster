//! An agent's context compacted at its prompt (MIP-5, section 10): the line the agent's manifest
//! gives in `[session] compact`, typed as [`super::typist`] types any line. A compaction asked
//! for mid-turn waits for the turn to end, which is also when a harness could act on it.
//!
//! It is taken once the prompt is empty again or the agent has gone to work on it, which a
//! compaction does and a rename does not. One given up is not typed again until somebody asks
//! again or the context crosses the threshold again.

use std::collections::HashSet;
use std::time::Instant;

use muster_core::diagnostics::log;
use muster_core::fields;

use super::doorbell::Moving;
use super::presence::{Panes, Seen};
use super::typist::{self, Line};
use crate::session::Shared;

/// A compaction typed and not yet seen taken, by pane, with its line.
pub(crate) type Compacting = typist::Typing<String>;

struct Compaction;

impl Line for Compaction {
    type What = String;
    const NOUN: &'static str = "compaction";
    const TYPED: &'static str = "daemon.compact.typed";
    const TAKEN_AT_WORK: bool = true;

    fn wanted(seen: &Seen) -> Option<(String, &str)> {
        seen.compact.as_deref().map(|line| (line.to_string(), line))
    }

    fn still_wanted(_: &Seen) -> bool {
        true
    }

    fn taken(shared: &Shared, pane: &str, line: &String) {
        log::info("daemon.compact.taken", fields! { "pane" => pane });
        shared.lock().compaction_typed(pane, line);
    }

    fn given_up(shared: &Shared, pane: &str, line: &String, why: &str) {
        log::warn(
            "daemon.compact.given_up",
            fields! {
                "pane" => pane,
                "why" => why,
                "impact" => "the agent's context is not compacted; nothing types the compaction \
                             again until somebody asks again or its context crosses compact_at \
                             again",
                "check" => "what the pane shows: the compaction may sit in the agent's prompt \
                            beside something typed after it, or a dialog may have opened over \
                            it; `muster pane compact` asks again",
            },
        );
        shared.lock().compaction_typed(pane, line);
    }
}

/// Types each pane's compaction still wanted into its agent's prompt, and looks again at each
/// typed earlier ([`typist::look`]).
pub(crate) fn look(
    shared: &Shared,
    panes: &Panes,
    busy: &HashSet<String>,
    now: Instant,
    compacting: &mut Compacting,
    moving: &mut Moving,
    next: &mut Option<Instant>,
) {
    typist::look::<Compaction>(shared, panes, busy, now, compacting, moving, next);
}
