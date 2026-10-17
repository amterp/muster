//! An agent's context compacted (MIP-5, section 10): asked for by `muster pane compact`, or by
//! this daemon once its agent reports a context past `compact_at`.
//!
//! Either way it is a line to type, the one the agent's manifest gives in `[session] compact`,
//! and the doorbell's thread types it at the agent's idle, empty prompt
//! (`messages/compacts.rs`). The daemon holds it until then rather than typing it at work, so it
//! does not depend on what a harness does with a line typed mid-turn.
//!
//! A compaction somebody asks for may carry a prompt to type once it has run, so an agent
//! compacted mid-task carries on rather than waiting at its prompt for somebody to type. It is
//! typed the same way, at the agent's next idle, empty prompt after the compaction was taken,
//! which is after the compaction has run: a harness compacting reads as working.
//!
//! The threshold compacts once per crossing. Reports arrive every few seconds while the context
//! stays past it, and a compaction that did not bring it back under must not be typed again on
//! each of them: the context has to fall below the threshold before it can cross it again.

/// One pane's compaction, still to be typed, and where its context stands against the threshold.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Compaction {
    wanted: Option<Wanted>,
    /// The prompt a compaction taken was asked to be followed by, still to be typed.
    follow_up: Option<String>,
    /// Its agent's context has crossed the threshold since it was last below it, so the
    /// threshold has had its one compaction for this crossing.
    crossed: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Wanted {
    line: String,
    by: By,
    then: Option<String>,
}

/// A line to type at the agent's prompt: its compaction, or the prompt owed once that has run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) line: String,
    pub(crate) follow_up: bool,
}

/// What a pane has queued, as its record says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pending {
    Nothing,
    Compaction,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Asked,
    Threshold,
}

impl Compaction {
    /// The next line to type at the agent's empty prompt, if one is still to be typed: a
    /// compaction before any prompt owed, since a compaction wanted now is newer than it.
    pub(crate) fn wanted(&self) -> Option<Step> {
        match (&self.wanted, &self.follow_up) {
            (Some(wanted), _) => Some(Step { line: wanted.line.clone(), follow_up: false }),
            (None, Some(then)) => Some(Step { line: then.clone(), follow_up: true }),
            (None, None) => None,
        }
    }

    pub(crate) fn pending(&self) -> Pending {
        match (&self.wanted, &self.follow_up) {
            (Some(_), _) => Pending::Compaction,
            (None, Some(_)) => Pending::FollowUp,
            (None, None) => Pending::Nothing,
        }
    }

    /// Somebody asked for the agent to be compacted, and then to be given `then`. It replaces a
    /// compaction not yet typed and a prompt still owed, since the later ask is the one its asker
    /// means.
    pub(crate) fn ask(&mut self, line: String, then: Option<String>) {
        self.wanted = Some(Wanted { line, by: By::Asked, then });
        self.follow_up = None;
    }

    /// The agent reported its context `used` percent full, against `threshold`. True when this
    /// report crossed it, and the threshold's compaction should be asked for now.
    pub(crate) fn reported(&mut self, used: f32, threshold: Option<f32>) -> bool {
        let past = threshold.is_some_and(|threshold| used >= threshold);
        let crossing = past && !self.crossed;
        self.crossed = past;
        crossing
    }

    /// The threshold's compaction, typed as `line`. An ask already waiting is left alone: it
    /// compacts the agent all the same, and keeps what its asker wanted kept.
    pub(crate) fn crossed(&mut self, line: String) {
        if self.wanted.is_none() {
            self.wanted = Some(Wanted { line, by: By::Threshold, then: None });
        }
    }

    /// Nothing compacts past a threshold any more: a compaction it asked for and nobody typed
    /// yet goes, and one somebody asked for stays.
    pub(crate) fn threshold_off(&mut self) {
        if self.wanted.as_ref().is_some_and(|wanted| wanted.by == By::Threshold) {
            self.wanted = None;
        }
        self.crossed = false;
    }

    /// `line` was typed and the agent took it. A compaction taken leaves the prompt it was asked
    /// to be followed by still to be typed. A different line wanted since, asked for while this
    /// one was being typed, is still to be typed.
    pub(crate) fn taken(&mut self, line: &str) {
        if self.wanted.as_ref().is_some_and(|wanted| wanted.line == line) {
            self.follow_up = self.wanted.take().and_then(|wanted| wanted.then);
        } else if self.follow_up.as_deref() == Some(line) {
            self.follow_up = None;
        }
    }

    /// Typing `line` was given up. A compaction given up takes its prompt with it, which would
    /// set the agent going on the context it was to be compacted out of; the prompt is returned,
    /// to say what was dropped.
    pub(crate) fn given_up(&mut self, line: &str) -> Option<String> {
        if self.wanted.as_ref().is_some_and(|wanted| wanted.line == line) {
            return self.wanted.take().and_then(|wanted| wanted.then);
        }
        if self.follow_up.as_deref() == Some(line) {
            self.follow_up = None;
        }
        None
    }

    /// The agent went, or its session started afresh: what was wanted of the last one is not
    /// this one's.
    pub(crate) fn cleared(&mut self) {
        *self = Compaction::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_threshold_compacts_once_per_crossing() {
        let mut compaction = Compaction::default();
        assert!(!compaction.reported(70.0, Some(80.0)));
        assert!(compaction.reported(80.0, Some(80.0)), "at the threshold is past it");
        assert!(!compaction.reported(95.0, Some(80.0)), "still past it is the same crossing");
        assert!(!compaction.reported(30.0, Some(80.0)));
        assert!(compaction.reported(85.0, Some(80.0)), "below and back is a new crossing");
        assert!(!compaction.reported(99.0, None), "no threshold crosses nothing");
        assert!(compaction.reported(99.0, Some(80.0)), "and turning one on starts afresh");
    }

    #[test]
    fn an_ask_outranks_the_threshold_and_survives_it_being_turned_off() {
        let mut compaction = Compaction::default();
        compaction.crossed("/compact".to_string());
        compaction.ask("/compact keep the notes".to_string(), None);
        compaction.crossed("/compact".to_string());
        assert_eq!(line(&compaction), Some("/compact keep the notes".to_string()));
        compaction.threshold_off();
        assert_eq!(line(&compaction), Some("/compact keep the notes".to_string()));

        compaction.taken("/compact");
        assert_eq!(
            line(&compaction),
            Some("/compact keep the notes".to_string()),
            "another line stays"
        );
        compaction.taken("/compact keep the notes");
        assert_eq!(compaction.wanted(), None);

        compaction.crossed("/compact".to_string());
        compaction.threshold_off();
        assert_eq!(compaction.wanted(), None, "the threshold's own goes with it");
    }

    fn line(compaction: &Compaction) -> Option<String> {
        compaction.wanted().map(|step| step.line)
    }

    #[test]
    fn a_compaction_taken_leaves_its_prompt_to_type_next() {
        let mut compaction = Compaction::default();
        compaction.ask("/compact".to_string(), Some("carry on".to_string()));
        assert_eq!(compaction.pending(), Pending::Compaction);
        compaction.taken("/compact");
        assert_eq!(
            compaction.wanted(),
            Some(Step { line: "carry on".to_string(), follow_up: true })
        );
        assert_eq!(compaction.pending(), Pending::FollowUp);
        compaction.taken("carry on");
        assert_eq!(compaction.pending(), Pending::Nothing);
    }

    #[test]
    fn a_compaction_given_up_takes_its_prompt_with_it() {
        let mut compaction = Compaction::default();
        compaction.ask("/compact".to_string(), Some("carry on".to_string()));
        assert_eq!(compaction.given_up("/compact"), Some("carry on".to_string()));
        assert_eq!(compaction.pending(), Pending::Nothing);
    }

    #[test]
    fn a_later_ask_replaces_a_prompt_still_owed() {
        let mut compaction = Compaction::default();
        compaction.ask("/compact".to_string(), Some("carry on".to_string()));
        compaction.taken("/compact");
        compaction.ask("/compact again".to_string(), None);
        compaction.taken("/compact again");
        assert_eq!(compaction.pending(), Pending::Nothing, "the first prompt went with its ask");
    }
}
