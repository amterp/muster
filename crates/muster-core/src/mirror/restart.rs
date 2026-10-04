//! A daemon that answered, and is not the run the mirror knew.
//!
//! Every check on a connection passes after a daemon restarts: it answers, its tabs and names
//! come back, and the window reads `connected`. What does not come back is every process in
//! those panes - a shell starts again in each directory, an agent only if the daemon resumes
//! it - and a daemon that could not read what it saved comes back with nothing at all. No
//! daemon can say what a window was showing before, so this is worked out here, from the panes
//! the mirror held against the snapshot of the new run (`docs/architecture.md`, degradation).

use std::fmt::Write as _;

use crate::mirror::backend::{Pane, PaneId};
use crate::roster::pane_label;

/// What a daemon restart cost, as the mirror saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restart {
    /// Panes the mirror held that came back from the daemon's saved state, every one of them
    /// running a process the new run started.
    pub started_again: usize,
    /// Panes the mirror held that did not come back, by id and by what they were called.
    pub lost: Vec<(PaneId, String)>,
    /// Panes among both that had an agent running: id, what the pane was called, and the
    /// agent.
    pub agents: Vec<(PaneId, String, String)>,
}

impl Restart {
    /// Compares the panes held before with the ones the new run holds. `None` when nothing
    /// the mirror held was affected: every pane carried on, as across a handover, or there
    /// was nothing held.
    ///
    /// `from_file` is whether the new run brought its panes back from its saved state. Without
    /// it a pane present under the same name was taken over with its process running.
    pub fn between(
        before: &[Pane],
        holds: impl Fn(&PaneId) -> bool,
        from_file: bool,
    ) -> Option<Restart> {
        let mut started_again = 0;
        let mut lost = Vec::new();
        let mut agents = Vec::new();
        for pane in before {
            let back = holds(&pane.id);
            if back && !from_file {
                continue;
            }
            let label = pane_label(pane);
            if back {
                started_again += 1;
            } else {
                lost.push((pane.id.clone(), label.clone()));
            }
            if let Some(agent) = pane.agent.as_deref().filter(|agent| !agent.is_empty()) {
                agents.push((pane.id.clone(), label, agent.to_string()));
            }
        }
        (started_again > 0 || !lost.is_empty()).then_some(Restart { started_again, lost, agents })
    }

    /// The whole sentence for the window's problem list: what happened, what it cost, and
    /// what to do about it.
    pub fn describe(&self, daemon: &str) -> String {
        let mut said = if self.started_again > 0 {
            let which = if self.started_again == 1 {
                "its pane".to_string()
            } else {
                format!("its {} panes", self.started_again)
            };
            format!(
                "{daemon} restarted, so {which} came back from its saved state with new processes: \
                 each shell starts again in its directory, and the scrollback is gone."
            )
        } else {
            format!(
                "{daemon} restarted with nothing saved to bring back, so {} it held did not come \
                 back.",
                panes(self.lost.len())
            )
        };
        if !self.agents.is_empty() {
            let running: Vec<&str> =
                self.agents.iter().map(|(_, label, _)| label.as_str()).collect();
            let _ = write!(
                said,
                " {} had an agent running ({}): Claude Code starts again in its session when \
                 `resume_agents` is on, and any other agent has to be started again.",
                panes(self.agents.len()),
                running.join(", ")
            );
        }
        if self.started_again > 0 && !self.lost.is_empty() {
            let names: Vec<&str> = self.lost.iter().map(|(_, label)| label.as_str()).collect();
            let _ = write!(
                said,
                " {} did not come back: {}. The daemon's log says why.",
                panes(self.lost.len()),
                names.join(", ")
            );
        }
        said
    }

    /// The same in a few words, for a machine's line in `muster window`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.started_again > 0 {
            parts.push(format!("{} started again", panes(self.started_again)));
        }
        if !self.lost.is_empty() {
            parts.push(format!("{} lost", self.lost.len()));
        }
        match self.agents.len() {
            0 => {}
            1 => parts.push("1 agent stopped".to_string()),
            agents => parts.push(format!("{agents} agents stopped")),
        }
        format!("restarted: {}", parts.join(", "))
    }
}

fn panes(count: usize) -> String {
    if count == 1 { "1 pane".to_string() } else { format!("{count} panes") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> (PaneId, String) {
        (PaneId::new(id), format!("{id}-dir"))
    }

    #[test]
    fn a_restart_says_what_came_back_what_stopped_and_what_did_not() {
        let restart = Restart {
            started_again: 3,
            lost: vec![pane("p4")],
            agents: vec![
                (PaneId::new("p1"), "api · claude".into(), "claude".into()),
                (PaneId::new("p2"), "web · codex".into(), "codex".into()),
            ],
        };
        assert_eq!(
            restart.describe("local"),
            "local restarted, so its 3 panes came back from its saved state with new processes: \
             each shell starts again in its directory, and the scrollback is gone. 2 panes had an \
             agent running (api · claude, web · codex): Claude Code starts again in its session \
             when `resume_agents` is on, and any other agent has to be started again. 1 pane did \
             not come back: p4-dir. The daemon's log says why."
        );
        assert_eq!(restart.summary(), "restarted: 3 panes started again, 1 lost, 2 agents stopped");
    }

    #[test]
    fn a_restart_with_nothing_saved_says_nothing_came_back() {
        let restart =
            Restart { started_again: 0, lost: vec![pane("p1"), pane("p2")], agents: vec![] };
        assert_eq!(
            restart.describe("devenv"),
            "devenv restarted with nothing saved to bring back, so 2 panes it held did not come back."
        );
        assert_eq!(restart.summary(), "restarted: 2 lost");
    }
}
