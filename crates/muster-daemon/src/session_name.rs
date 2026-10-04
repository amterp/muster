//! A pane's name kept in step with the name its agent's harness gives the session (MIP-5,
//! section 10), both ways.
//!
//! A pane renamed by a person or a script hands its name to the session: the doorbell's thread
//! types the line the agent's manifest gives for it (`[session] rename`) at its empty prompt
//! (`messages/renames.rs`). A session renamed in its harness hands its name to the pane: the harness's
//! adapter reports it (`muster-daemon report --session-name`), and the pane takes it.
//!
//! Each direction must not set off the other. A name the pane took from the harness is never
//! typed back, a name the harness says again is not news, and a name the harness reports that
//! the pane already has ends any typing still to come. The one judgement is the first name a
//! session reports: it is the name the session started with, a resumed one's say, not a rename,
//! so a pane with a name of its own keeps it and hands it on, and a pane without one takes it.

use muster_core::diagnostics::log;
use muster_core::fields;

/// The longest name typed into a session. A pane's name is never typed in part.
const NAME_BYTES: usize = 128;

/// One pane's side of the bargain.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SessionName {
    /// What the session's harness last said the session is called, empty when it said it has
    /// no name; none when it has said nothing since its agent came or its facts were cleared.
    heard: Option<String>,
    /// The pane's name, still to be typed into the session.
    wanted: Option<String>,
    /// The last name typed into the session and taken there, which is not typed again while
    /// it stands.
    typed: Option<String>,
}

impl SessionName {
    /// The pane's name, to be typed into the session once its agent is at an empty prompt.
    pub(crate) fn wanted(&self) -> Option<&str> {
        self.wanted.as_deref()
    }

    /// A person or a script named the pane, or took its name away.
    pub(crate) fn named(&mut self, pane: &str, label: Option<&str>) {
        self.want(pane, label);
    }

    /// The harness said its session is called `said`, empty for no name. Answers the name the
    /// pane takes, when it takes one.
    pub(crate) fn heard(&mut self, pane: &str, said: &str, label: Option<&str>) -> Option<String> {
        if self.heard.as_deref() == Some(said) {
            return None;
        }
        let first = self.heard.is_none();
        self.heard = Some(said.to_string());
        if said.is_empty() || label == Some(said) {
            self.want(pane, label);
            return None;
        }
        if first && label.is_some() {
            self.want(pane, label);
            return None;
        }
        // What was typed before no longer stands: naming the pane that again has to type it.
        self.wanted = None;
        self.typed = None;
        Some(said.to_string())
    }

    /// The agent in the pane changed: `replaced` when another agent, or none, took the place of
    /// one that was there, rather than an agent coming to a pane that had none. A statusline
    /// can report before detection has found its agent, and what it said then is the new
    /// agent's.
    pub(crate) fn agent_changed(&mut self, pane: &str, replaced: bool, label: Option<&str>) {
        if replaced {
            self.heard = None;
            self.typed = None;
        }
        self.want(pane, label);
    }

    /// The agent's facts were cleared, as a session starting afresh clears them: what its harness
    /// said before is the last session's.
    pub(crate) fn cleared(&mut self, pane: &str, label: Option<&str>) {
        self.heard = None;
        self.typed = None;
        self.want(pane, label);
    }

    /// `name` was typed into the session and taken, or typing it was given up; either way it is
    /// not typed again while it stands.
    pub(crate) fn typed(&mut self, name: &str) {
        self.typed = Some(name.to_string());
        if self.wanted.as_deref() == Some(name) {
            self.wanted = None;
        }
    }

    fn want(&mut self, pane: &str, label: Option<&str>) {
        self.wanted = label
            .filter(|label| {
                self.heard.as_deref() != Some(*label) && self.typed.as_deref() != Some(*label)
            })
            .filter(|label| typeable(pane, label))
            .map(str::to_string);
    }
}

/// Whether a name can be typed into a prompt as part of one line: nothing in it may act as a
/// key, and it must fit.
fn typeable(pane: &str, label: &str) -> bool {
    let why = if label.chars().any(char::is_control) {
        "it holds a control character, which a prompt would take as a key"
    } else if label.len() > NAME_BYTES {
        "it is longer than a session's name is kept"
    } else {
        return true;
    };
    log::warn(
        "session.name.untypeable",
        fields! {
            "pane" => pane,
            "bytes" => label.len(),
            "why" => why,
            "impact" => "the pane's name is not given to the session its agent runs",
            "check" => format!("rename the pane to a name of at most {NAME_BYTES} bytes, with no control characters"),
        },
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(state: &mut SessionName, said: &str, label: Option<&str>) -> Option<String> {
        state.heard("p1", said, label)
    }

    #[test]
    fn a_pane_named_by_a_person_hands_its_name_to_the_session_once() {
        let mut state = SessionName::default();
        state.named("p1", Some("🤖 A"));
        assert_eq!(state.wanted(), Some("🤖 A"));
        state.typed("🤖 A");
        assert_eq!(state.wanted(), None);
        state.named("p1", Some("🤖 A"));
        assert_eq!(state.wanted(), None, "typed already");
    }

    #[test]
    fn taking_a_panes_name_away_types_nothing() {
        let mut state = SessionName::default();
        state.named("p1", Some("A"));
        state.named("p1", None);
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_rename_in_the_harness_renames_the_pane_and_is_not_typed_back() {
        let mut state = SessionName::default();
        assert_eq!(heard(&mut state, "", None), None);
        assert_eq!(heard(&mut state, "B", None).as_deref(), Some("B"));
        assert_eq!(state.wanted(), None);
        // The pane took B, which the daemon does without calling `named`.
        assert_eq!(heard(&mut state, "B", Some("B")), None, "said again");
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_pane_named_back_after_a_rename_in_the_harness_hands_the_name_on_again() {
        let mut state = SessionName::default();
        heard(&mut state, "", None);
        state.named("p1", Some("A"));
        state.typed("A");
        heard(&mut state, "A", Some("A"));
        assert_eq!(heard(&mut state, "B", Some("A")).as_deref(), Some("B"));
        state.named("p1", Some("A"));
        assert_eq!(state.wanted(), Some("A"), "the session is called B now, not A");
    }

    #[test]
    fn a_rename_in_the_harness_ends_typing_still_to_come() {
        let mut state = SessionName::default();
        heard(&mut state, "", None);
        state.named("p1", Some("A"));
        assert_eq!(state.wanted(), Some("A"));
        assert_eq!(heard(&mut state, "B", Some("A")).as_deref(), Some("B"));
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn the_first_name_a_session_reports_gives_an_unnamed_pane_its_name() {
        let mut state = SessionName::default();
        assert_eq!(heard(&mut state, "resumed", None).as_deref(), Some("resumed"));
    }

    #[test]
    fn the_first_name_a_session_reports_gives_way_to_a_name_the_pane_has() {
        let mut state = SessionName::default();
        state.agent_changed("p1", false, Some("A"));
        assert_eq!(heard(&mut state, "resumed", Some("A")), None);
        assert_eq!(state.wanted(), Some("A"));
        state.typed("A");
        assert_eq!(heard(&mut state, "A", Some("A")), None);
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_session_that_says_it_has_no_name_is_given_the_panes() {
        let mut state = SessionName::default();
        assert_eq!(heard(&mut state, "", Some("A")), None);
        assert_eq!(state.wanted(), Some("A"));
    }

    #[test]
    fn a_session_already_called_what_the_pane_is_called_is_typed_nothing() {
        let mut state = SessionName::default();
        heard(&mut state, "A", None);
        state.named("p1", Some("A"));
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_new_agent_in_a_named_pane_is_given_the_name_again() {
        let mut state = SessionName::default();
        state.named("p1", Some("A"));
        state.typed("A");
        heard(&mut state, "A", Some("A"));
        state.agent_changed("p1", true, Some("A"));
        assert_eq!(state.wanted(), Some("A"));
    }

    #[test]
    fn an_agent_found_after_its_statusline_spoke_keeps_what_it_said() {
        let mut state = SessionName::default();
        heard(&mut state, "A", Some("A"));
        state.agent_changed("p1", false, Some("A"));
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_cleared_session_is_given_the_name_again() {
        let mut state = SessionName::default();
        heard(&mut state, "A", Some("A"));
        state.cleared("p1", Some("A"));
        assert_eq!(state.wanted(), Some("A"));
        assert_eq!(heard(&mut state, "A", Some("A")), None);
        assert_eq!(state.wanted(), None);
    }

    #[test]
    fn a_name_that_cannot_be_typed_is_not_wanted() {
        let mut state = SessionName::default();
        state.named("p1", Some("two\nlines"));
        assert_eq!(state.wanted(), None);
        state.named("p1", Some(&"x".repeat(NAME_BYTES + 1)));
        assert_eq!(state.wanted(), None);
    }
}
