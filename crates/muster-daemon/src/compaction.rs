//! An agent's context compacted (MIP-5, section 10): asked for by `muster pane compact`, or by
//! this daemon once its agent reports a context past `compact_at`.
//!
//! Either way it is a line to type, the one the agent's manifest gives in `[session] compact`,
//! and the doorbell's thread types it at the agent's idle, empty prompt
//! (`messages/compacts.rs`). The daemon holds it until then rather than typing it at work, so it
//! does not depend on what a harness does with a line typed mid-turn.
//!
//! The threshold compacts once per crossing. Reports arrive every few seconds while the context
//! stays past it, and a compaction that did not bring it back under must not be typed again on
//! each of them: the context has to fall below the threshold before it can cross it again.

/// One pane's compaction, still to be typed, and where its context stands against the threshold.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Compaction {
    wanted: Option<Wanted>,
    /// Its agent's context has crossed the threshold since it was last below it, so the
    /// threshold has had its one compaction for this crossing.
    crossed: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Wanted {
    line: String,
    by: By,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Asked,
    Threshold,
}

impl Compaction {
    /// The line to type at the agent's empty prompt, if one is still to be typed.
    pub(crate) fn wanted(&self) -> Option<&str> {
        self.wanted.as_ref().map(|wanted| wanted.line.as_str())
    }

    /// Somebody asked for the agent to be compacted. It replaces a compaction not yet typed,
    /// since the later ask is the one its asker means.
    pub(crate) fn ask(&mut self, line: String) {
        self.wanted = Some(Wanted { line, by: By::Asked });
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
            self.wanted = Some(Wanted { line, by: By::Threshold });
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

    /// `line` was typed and taken, or typing it was given up. A different line wanted since,
    /// asked for while this one was being typed, is still to be typed.
    pub(crate) fn typed(&mut self, line: &str) {
        if self.wanted() == Some(line) {
            self.wanted = None;
        }
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
        compaction.ask("/compact keep the notes".to_string());
        compaction.crossed("/compact".to_string());
        assert_eq!(compaction.wanted(), Some("/compact keep the notes"));
        compaction.threshold_off();
        assert_eq!(compaction.wanted(), Some("/compact keep the notes"));

        compaction.typed("/compact");
        assert_eq!(compaction.wanted(), Some("/compact keep the notes"), "another line stays");
        compaction.typed("/compact keep the notes");
        assert_eq!(compaction.wanted(), None);

        compaction.crossed("/compact".to_string());
        compaction.threshold_off();
        assert_eq!(compaction.wanted(), None, "the threshold's own goes with it");
    }
}
