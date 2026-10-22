//! An agent's context compacted at its prompt (MIP-5, section 10): the line the agent's manifest
//! gives in `[session] compact`, typed as [`super::typist`] types any line. A compaction asked
//! for mid-turn waits for the turn to end, which is also when a harness could act on it.
//!
//! It is taken once the prompt is empty again or the agent has gone to work on it, which a
//! compaction does and a rename does not. One given up is not typed again until somebody asks
//! again or the context crosses the threshold again.
//!
//! A compaction asked to be followed by a prompt has the prompt typed the same way once it has
//! been taken, at the agent's next idle, empty prompt: after the compaction has run, since an
//! agent compacting reads as working.

use std::collections::HashSet;
use std::time::Instant;

use muster_core::diagnostics::log;
use muster_core::fields;

use super::doorbell::Moving;
use super::presence::{Panes, Seen};
use super::typist::{self, Line};
use crate::compaction::Step;
use crate::session::Shared;

/// A compaction, or the prompt owed after it, typed and not yet seen taken, by pane.
pub(crate) type Compacting = typist::Typing<Step>;

struct Compaction;

impl Line for Compaction {
    type What = Step;
    const NOUN: &'static str = "compaction";
    const TYPED: &'static str = "daemon.compact.typed";
    const TAKEN_AT_WORK: bool = true;

    /// The prompt it is followed by is somebody's work, which the agent goes to.
    fn chore(step: &Step) -> bool {
        !step.follow_up
    }

    fn wanted(seen: &Seen) -> Option<(Step, &str)> {
        seen.compact.as_ref().map(|step| (step.clone(), step.line.as_str()))
    }

    fn which(step: &Step) -> &'static str {
        if step.follow_up { "follow_up" } else { "compaction" }
    }

    fn still_wanted(_: &Seen) -> bool {
        true
    }

    fn taken(shared: &Shared, pane: &str, step: &Step) {
        log::info("daemon.compact.taken", fields! { "pane" => pane, "line" => Self::which(step) });
        shared.lock().compaction_taken(pane, &step.line);
    }

    fn given_up(shared: &Shared, pane: &str, step: &Step, why: &str) {
        if step.follow_up {
            log::warn(
                "daemon.compact.follow_up_given_up",
                fields! {
                    "pane" => pane,
                    "why" => why,
                    "impact" => "the agent was compacted and was not given the prompt it was to \
                                 carry on with, so it waits at its prompt",
                    "check" => "what the pane shows: the prompt may sit unsent beside something \
                                typed after it; `muster pane send` can type it",
                },
            );
            shared.lock().compaction_given_up(pane, &step.line);
            return;
        }
        let dropped = shared.lock().compaction_given_up(pane, &step.line).is_some();
        log::warn(
            "daemon.compact.given_up",
            fields! {
                "pane" => pane,
                "why" => why,
                "follow_up_dropped" => dropped,
                "impact" => "the agent's context is not compacted, and a prompt it was to carry \
                             on with afterwards is dropped with it; nothing types the compaction \
                             again until somebody asks again or its context crosses compact_at \
                             again",
                "check" => "what the pane shows: the compaction may sit in the agent's prompt \
                            beside something typed after it, or a dialog may have opened over \
                            it; `muster pane compact` asks again",
            },
        );
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
