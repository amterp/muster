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
//!
//! A second threshold, lower, has the agent told to compact itself instead: a nudge, posted as a
//! message (`messages/compact_nudges.rs`). Once per crossing too, and counted apart from the
//! compaction's, since an agent can be past one and not the other.

/// One pane's compaction, still to be typed, and where its context stands against the threshold.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Compaction {
    wanted: Option<Wanted>,
    /// The prompt a compaction taken was asked to be followed by, still to be typed.
    follow_up: Option<String>,
    /// Its agent's context has crossed the threshold since it was last below it, so the
    /// threshold has had its one compaction for this crossing.
    crossed: bool,
    /// How full its agent last said its context was.
    used: Option<f32>,
    /// A nudge still to be posted.
    nudge: Option<Nudge>,
    /// The agent has been posted its nudge since its context was last below the nudge's
    /// threshold.
    nudged: bool,
    /// Posting the nudge for this crossing has failed and been said to have.
    nudge_refused: bool,
}

/// A nudge owed to a pane's agent: how full it said its context was, the percent that is past,
/// and the percent it is compacted at, if it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Nudge {
    pub(crate) used: f32,
    pub(crate) at: u8,
    pub(crate) compact_at: Option<u8>,
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
        // The agent is about to be compacted, so a nudge not yet posted would have it do it twice.
        self.nudge = None;
    }

    /// The agent reported its context `used` percent full, against `threshold`. True when this
    /// report crossed it, and the threshold's compaction should be asked for now.
    pub(crate) fn reported(&mut self, used: f32, threshold: Option<f32>) -> bool {
        self.used = Some(used);
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

    /// The agent reported its context `used` percent full, against the percent it is nudged
    /// `at`, none when it is not to be nudged. A nudge is owed when that is past and this
    /// crossing has had none, unless a compaction waits to be typed: the agent is about to be
    /// compacted, and told to compact itself it would do it twice.
    pub(crate) fn nudge_reported(&mut self, used: f32, at: Option<u8>, compact_at: Option<u8>) {
        let Some(at) = at.filter(|at| used >= f32::from(*at)) else {
            self.nudge = None;
            self.nudged = false;
            self.nudge_refused = false;
            return;
        };
        self.nudge =
            (!self.nudged && self.wanted.is_none()).then_some(Nudge { used, at, compact_at });
    }

    pub(crate) fn nudge_due(&self) -> Option<Nudge> {
        self.nudge
    }

    /// The pane is nudged `at` this percent now, none when it is not to be. A context known to
    /// be under the new line has not crossed it: a nudge owed for the old one goes, and the agent
    /// can be nudged when it does cross. One still owed past the new line names the new percent.
    /// While no context has been reported, as in a pane just taken over, nothing is known to
    /// have changed, and what a handover carried stands.
    pub(crate) fn nudge_threshold_is(&mut self, at: Option<u8>) {
        let past = match (at, self.used) {
            (Some(at), Some(used)) => used >= f32::from(at),
            (Some(_), None) => return,
            (None, _) => false,
        };
        if !past {
            self.nudge = None;
            self.nudged = false;
            self.nudge_refused = false;
        } else if let (Some(nudge), Some(at)) = (&mut self.nudge, at) {
            nudge.at = at;
        }
    }

    /// The nudge owed was posted.
    pub(crate) fn nudge_posted(&mut self) {
        self.nudge = None;
        self.nudged = true;
    }

    /// The nudge owed could not be posted, and is owed again at the next report. True the first
    /// time for this crossing.
    pub(crate) fn nudge_not_posted(&mut self) -> bool {
        self.nudge = None;
        !std::mem::replace(&mut self.nudge_refused, true)
    }

    /// Whether the agent has had its nudge for the crossing its context is past, which a
    /// handover carries.
    pub(crate) fn nudged(&self) -> bool {
        self.nudged
    }

    /// As a pane handed over arrives: nudged already, or not.
    pub(crate) fn carry_nudged(&mut self, nudged: bool) {
        self.nudged = nudged;
    }

    /// The pane's threshold is `threshold` now. A compaction the old one asked for and nobody
    /// typed yet goes when the context is not past the new one, as when there is none.
    pub(crate) fn threshold_is(&mut self, threshold: Option<f32>) {
        let past = threshold.zip(self.used).is_some_and(|(threshold, used)| used >= threshold);
        if !past {
            self.threshold_off();
        }
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
    fn a_nudge_is_owed_once_per_crossing() {
        let mut compaction = Compaction::default();
        compaction.nudge_reported(50.0, Some(60), Some(85));
        assert_eq!(compaction.nudge_due(), None, "under the line");
        compaction.nudge_reported(62.0, Some(60), Some(85));
        let owed = Nudge { used: 62.0, at: 60, compact_at: Some(85) };
        assert_eq!(compaction.nudge_due(), Some(owed));
        compaction.nudge_posted();
        compaction.nudge_reported(70.0, Some(60), Some(85));
        assert_eq!(compaction.nudge_due(), None, "nudged for this crossing already");
        compaction.nudge_reported(30.0, Some(60), Some(85));
        compaction.nudge_reported(61.0, Some(60), Some(85));
        assert!(compaction.nudge_due().is_some(), "it fell below and crossed again");
    }

    #[test]
    fn a_nudge_that_could_not_be_posted_is_owed_again_and_said_once() {
        let mut compaction = Compaction::default();
        compaction.nudge_reported(62.0, Some(60), None);
        assert!(compaction.nudge_not_posted(), "said the first time");
        assert_eq!(compaction.nudge_due(), None);
        compaction.nudge_reported(63.0, Some(60), None);
        assert!(compaction.nudge_due().is_some(), "owed again at the next report");
        assert!(!compaction.nudge_not_posted(), "and not said again");
    }

    #[test]
    fn no_nudge_is_owed_while_a_compaction_waits_or_with_no_threshold() {
        let mut compaction = Compaction::default();
        compaction.crossed("/compact".to_string());
        compaction.nudge_reported(90.0, Some(60), Some(85));
        assert_eq!(compaction.nudge_due(), None, "about to be compacted");
        compaction.taken("/compact");
        compaction.nudge_reported(90.0, Some(60), Some(85));
        assert!(compaction.nudge_due().is_some(), "still past it once the compaction was typed");
        compaction.nudge_reported(90.0, None, Some(85));
        assert_eq!(compaction.nudge_due(), None, "the threshold went");
    }

    #[test]
    fn a_threshold_raised_past_the_context_can_be_crossed_and_nudged_again() {
        let mut compaction = Compaction::default();
        compaction.reported(65.0, None);
        compaction.nudge_reported(65.0, Some(60), None);
        compaction.nudge_posted();
        compaction.nudge_threshold_is(Some(80));
        compaction.nudge_reported(85.0, Some(80), None);
        assert!(compaction.nudge_due().is_some(), "65 had not crossed 80");

        compaction.reported(85.0, None);
        compaction.nudge_threshold_is(Some(70));
        assert_eq!(compaction.nudge_due().map(|nudge| nudge.at), Some(70), "still past it");
        compaction.nudge_threshold_is(None);
        assert_eq!(compaction.nudge_due(), None, "the threshold went before it was posted");
    }

    #[test]
    fn a_compaction_asked_for_takes_back_a_nudge_not_yet_posted() {
        let mut compaction = Compaction::default();
        compaction.nudge_reported(62.0, Some(60), None);
        compaction.ask("/compact".to_string(), None);
        assert_eq!(compaction.nudge_due(), None);
    }

    #[test]
    fn a_new_session_of_the_context_can_be_nudged_again() {
        let mut compaction = Compaction::default();
        compaction.nudge_reported(62.0, Some(60), None);
        compaction.nudge_posted();
        compaction.cleared();
        compaction.nudge_reported(61.0, Some(60), None);
        assert!(compaction.nudge_due().is_some());
    }

    #[test]
    fn a_handover_carries_that_the_agent_was_nudged() {
        let mut old = Compaction::default();
        old.nudge_reported(62.0, Some(60), None);
        old.nudge_posted();
        let mut new = Compaction::default();
        new.carry_nudged(old.nudged());
        // As a report is handled: the thresholds are worked out again before the context it
        // carries is noted, so the first one after a handover is judged with none known.
        new.nudge_threshold_is(Some(60));
        new.reported(63.0, None);
        new.nudge_reported(63.0, Some(60), None);
        assert_eq!(new.nudge_due(), None);
        new.nudge_threshold_is(Some(60));
        assert!(new.nudged(), "a refresh with the context known and still past it");
    }

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
