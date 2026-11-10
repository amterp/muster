//! An agent told to compact itself, once its context passes the `nudge_at` that applies to its
//! pane ([`crate::compaction`]).
//!
//! The telling is a message from `@muster` in a group of the agent's own, so it wakes the agent
//! at its next idle, empty prompt as any message does, and an agent at work reads it when its
//! turn ends. It is posted from the doorbell's thread rather than where the report is handled,
//! because a post takes the messaging lock and the report is handled under the session's.
//!
//! An agent can pass both thresholds in one turn, be compacted at its next prompt, and only then
//! read a nudge posted before. A message cannot be taken back, so the text says to ignore it if
//! the agent has compacted since.

use std::sync::Arc;

use muster_core::diagnostics::log;
use muster_core::fields;

use super::presence::Panes;
use super::{delivering, now_ms, told};
use crate::session::Shared;

/// A nudge owed to the agent in `pane`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Due {
    pub(crate) pane: String,
    pub(crate) agent: String,
    /// How full the agent last said its context was, in percent.
    pub(crate) used: f32,
    pub(crate) nudge_at: u8,
    /// What set `nudge_at`, as a log line names it.
    pub(crate) source: &'static str,
    /// The percent Muster compacts the agent at itself, if it does.
    pub(crate) compact_at: Option<u8>,
}

/// Posts every nudge owed. One that cannot be posted is owed again at the agent's next report.
pub(super) fn look(shared: &Arc<Shared>) {
    let due = shared.lock().nudges_due();
    if due.is_empty() {
        return;
    }
    let panes = Panes::of(shared);
    for nudge in due {
        let body = body(&nudge);
        let delivered = delivering(shared, &panes, |service| {
            service.tell_alone_as_muster(&nudge.pane, &body, &panes, now_ms())
        });
        match delivered {
            Ok(delivered) => {
                told("msg.compact_nudge.told", Some(body.len()), &delivered);
                shared.lock().nudged(&nudge.pane);
                log::info(
                    "daemon.compact.nudged",
                    fields! {
                        "pane" => nudge.pane,
                        "agent" => nudge.agent,
                        "context_used" => nudge.used,
                        "threshold" => nudge.nudge_at,
                        "source" => nudge.source,
                    },
                );
            }
            Err(reply) => {
                if shared.lock().nudge_not_posted(&nudge.pane) {
                    log::warn(
                        "daemon.compact.nudge_not_posted",
                        fields! {
                            "pane" => nudge.pane,
                            "agent" => nudge.agent,
                            "why" => reply.reason,
                            "impact" => "the agent was not told to compact itself; it is tried \
                                         again on each report of its context while that stays \
                                         past the threshold",
                            "check" => "whether the daemon is handing over, and whether \
                                        messages reach this pane at all (`muster msg post`)",
                        },
                    );
                }
            }
        }
    }
}

fn body(nudge: &Due) -> String {
    let backstop = nudge
        .compact_at
        .map(|compact_at| format!(" Muster compacts you itself at {compact_at}%."))
        .unwrap_or_default();
    format!(
        "Your context is {:.0}% full, past the {}% this pane is set to be reminded at. If you \
         have not compacted since this was sent, compact yourself at your next clean break: \
         `muster pane compact <what to keep> --then <prompt to carry on>`.{backstop}",
        nudge.used, nudge.nudge_at
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn due(compact_at: Option<u8>) -> Due {
        Due {
            pane: "p1".to_string(),
            agent: "claude".to_string(),
            used: 61.6,
            nudge_at: 60,
            source: "root",
            compact_at,
        }
    }

    #[test]
    fn the_nudge_gives_the_command_and_says_when_to_ignore_it() {
        assert_eq!(
            body(&due(None)),
            "Your context is 62% full, past the 60% this pane is set to be reminded at. If you \
             have not compacted since this was sent, compact yourself at your next clean break: \
             `muster pane compact <what to keep> --then <prompt to carry on>`."
        );
    }

    #[test]
    fn it_names_the_compaction_muster_does_itself_only_when_there_is_one() {
        assert!(body(&due(Some(85))).ends_with(" Muster compacts you itself at 85%."));
    }
}
