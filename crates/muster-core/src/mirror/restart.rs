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
    /// Panes that had an agent running and came back with the daemon resuming its session: id,
    /// what the pane was called, and the agent. The daemon says so by starting the pane with
    /// the resume as its command, which a pane it brought back otherwise never has.
    pub resumed: Vec<(PaneId, String, String)>,
    /// Panes that had an agent running and came back without it, or did not come back.
    pub stopped: Vec<(PaneId, String, String)>,
}

impl Restart {
    /// Compares the panes held before with the ones the new run holds. `None` when nothing
    /// the mirror held was affected: every pane carried on, as across a handover, or there
    /// was nothing held.
    ///
    /// `now` is the pane the new run holds under the same name, if any. `from_file` is whether
    /// the new run brought its panes back from its saved state; without it a pane present
    /// under the same name was taken over with its process running.
    pub fn between<'a>(
        before: &[Pane],
        now: impl Fn(&PaneId) -> Option<&'a Pane>,
        from_file: bool,
    ) -> Option<Restart> {
        let mut started_again = 0;
        let mut lost = Vec::new();
        let mut resumed = Vec::new();
        let mut stopped = Vec::new();
        for pane in before {
            let back = now(&pane.id);
            if back.is_some() && !from_file {
                continue;
            }
            let label = pane_label(pane);
            match back {
                Some(_) => started_again += 1,
                None => lost.push((pane.id.clone(), label.clone())),
            }
            let Some(agent) = pane.agent.as_deref().filter(|agent| !agent.is_empty()) else {
                continue;
            };
            let entry = (pane.id.clone(), label, agent.to_string());
            if back
                .and_then(|back| back.command.as_deref())
                .is_some_and(|command| !command.is_empty())
            {
                resumed.push(entry);
            } else {
                stopped.push(entry);
            }
        }
        (started_again > 0 || !lost.is_empty()).then_some(Restart {
            started_again,
            lost,
            resumed,
            stopped,
        })
    }

    /// Whether this restart still needs somebody: a pane that had an agent is still in the
    /// window and has not shown one since - stopped, or resumed and not yet found running - or
    /// the daemon came back with none of what it held and still holds nothing.
    ///
    /// A pane that did not come back holds nothing up - it is not in the window, so there is
    /// nothing to close or wait on - and nor does a shell that only started again, whose
    /// scrollback nothing can give back. Closing a pane settles it, since a warning about a
    /// pane the window no longer has helps nobody.
    ///
    /// `agent_of` is a pane's agent now: `None` for a pane the window does not hold, `Some(None)`
    /// for one running no agent.
    pub fn outstanding<'a>(
        &self,
        agent_of: impl Fn(&PaneId) -> Option<Option<&'a str>>,
        holds_any: bool,
    ) -> bool {
        let waiting = self
            .resumed
            .iter()
            .chain(&self.stopped)
            .any(|(pane, _, _)| agent_of(pane) == Some(None));
        waiting || (self.started_again == 0 && !holds_any)
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
        match self.resumed.len() {
            0 => {}
            1 => {
                let _ = write!(
                    said,
                    " The daemon resumed 1 agent in its session ({}).",
                    labels(&self.resumed)
                );
            }
            count => {
                let _ = write!(
                    said,
                    " The daemon resumed {count} agents in their sessions ({}).",
                    labels(&self.resumed)
                );
            }
        }
        match self.stopped.len() {
            0 => {}
            1 => {
                let _ = write!(
                    said,
                    " 1 agent stopped and has to be started again ({}).",
                    labels(&self.stopped)
                );
            }
            count => {
                let _ = write!(
                    said,
                    " {count} agents stopped and have to be started again ({}).",
                    labels(&self.stopped)
                );
            }
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
        for (count, what) in [(self.resumed.len(), "resumed"), (self.stopped.len(), "stopped")] {
            match count {
                0 => {}
                1 => parts.push(format!("1 agent {what}")),
                count => parts.push(format!("{count} agents {what}")),
            }
        }
        format!("restarted: {}", parts.join(", "))
    }
}

fn labels(agents: &[(PaneId, String, String)]) -> String {
    agents.iter().map(|(_, label, _)| label.as_str()).collect::<Vec<_>>().join(", ")
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
            resumed: vec![(PaneId::new("p1"), "api · claude".into(), "claude".into())],
            stopped: vec![(PaneId::new("p2"), "web · codex".into(), "codex".into())],
        };
        assert_eq!(
            restart.describe("local"),
            "local restarted, so its 3 panes came back from its saved state with new processes: \
             each shell starts again in its directory, and the scrollback is gone. The daemon \
             resumed 1 agent in its session (api · claude). 1 agent stopped and has to be started \
             again (web · codex). 1 pane did not come back: p4-dir. The daemon's log says why."
        );
        assert_eq!(
            restart.summary(),
            "restarted: 3 panes started again, 1 lost, 1 agent resumed, 1 agent stopped"
        );
    }

    #[test]
    fn a_restart_with_nothing_saved_says_nothing_came_back() {
        let restart = Restart {
            started_again: 0,
            lost: vec![pane("p1"), pane("p2")],
            resumed: vec![],
            stopped: vec![],
        };
        assert_eq!(
            restart.describe("devenv"),
            "devenv restarted with nothing saved to bring back, so 2 panes it held did not come back."
        );
        assert_eq!(restart.summary(), "restarted: 2 lost");
    }
}
