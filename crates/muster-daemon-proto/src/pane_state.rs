//! The state a pane's agent is shown in, worked out from its record one way for every reader:
//! the daemon dating it and serving it to other programs, and the CLI answering with no window.

use crate::{AgentState, Pane};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneState {
    /// No agent in the pane, or none read yet.
    Unknown,
    Working,
    Blocked,
    Idle,
    /// Idle, having said what it waits on.
    Waiting,
    /// The agent's program has stopped reading its input, whatever detection last read.
    Stuck,
    /// It finished something nobody has seen.
    Done,
}

impl Pane {
    /// What detection read, with stuck laid over it while the pane has an agent whose program
    /// has stopped reading: the state `state_since_ms` dates. Never waiting or done.
    pub fn state(&self) -> PaneState {
        if self.agent.is_some() && self.unread_input.is_some() {
            return PaneState::Stuck;
        }
        match self.agent_state() {
            AgentState::Working => PaneState::Working,
            AgentState::Blocked => PaneState::Blocked,
            AgentState::Idle => PaneState::Idle,
            AgentState::Unknown => PaneState::Unknown,
        }
    }

    /// The state a view shows: [`Pane::state`], with waiting over an idle agent that said what
    /// it waits on, and done over a finish nobody has seen unless the agent is busy again. A
    /// window also clears done for a pane it shows with the keyboard, which only it can know.
    pub fn shown_state(&self) -> PaneState {
        let waiting = self.facts.as_ref().is_some_and(|facts| facts.waiting.is_some());
        let state = match self.state() {
            PaneState::Idle if waiting => PaneState::Waiting,
            state => state,
        };
        match state {
            PaneState::Idle | PaneState::Unknown if self.finished_unseen => PaneState::Done,
            state => state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentFacts, UnreadInput};

    fn pane(state: AgentState) -> Pane {
        let mut pane = Pane { agent: Some("claude".to_string()), ..Pane::default() };
        pane.set_agent_state(state);
        pane
    }

    fn waiting(mut pane: Pane) -> Pane {
        pane.facts =
            Some(AgentFacts { waiting: Some("a build".to_string()), ..AgentFacts::default() });
        pane
    }

    fn stuck(mut pane: Pane) -> Pane {
        pane.unread_input = Some(UnreadInput::default());
        pane
    }

    fn finished(mut pane: Pane) -> Pane {
        pane.finished_unseen = true;
        pane
    }

    #[test]
    fn stuck_lies_over_what_was_read_and_only_with_an_agent() {
        assert_eq!(stuck(pane(AgentState::Working)).state(), PaneState::Stuck);
        let shell = Pane { agent: None, ..stuck(pane(AgentState::Unknown)) };
        assert_eq!(shell.state(), PaneState::Unknown, "a shell is never stuck");
    }

    #[test]
    fn waiting_and_done_are_shown_but_not_dated() {
        assert_eq!(waiting(pane(AgentState::Idle)).state(), PaneState::Idle);
        assert_eq!(waiting(pane(AgentState::Idle)).shown_state(), PaneState::Waiting);
        assert_eq!(finished(pane(AgentState::Idle)).state(), PaneState::Idle);
        assert_eq!(finished(pane(AgentState::Idle)).shown_state(), PaneState::Done);
        let gone = Pane { agent: None, ..finished(pane(AgentState::Unknown)) };
        assert_eq!(gone.shown_state(), PaneState::Done, "an agent that ended finished");
    }

    #[test]
    fn a_busy_agent_is_not_done_whatever_it_finished_before() {
        for busy in [
            finished(pane(AgentState::Working)),
            finished(pane(AgentState::Blocked)),
            finished(waiting(pane(AgentState::Idle))),
            finished(stuck(pane(AgentState::Idle))),
        ] {
            assert_ne!(busy.shown_state(), PaneState::Done, "{busy:?}");
        }
    }
}
