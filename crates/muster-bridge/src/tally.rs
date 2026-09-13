//! Frames counted between the lines that report them.
//!
//! A bridge says how much it painted once per interval rather than once per frame, because at
//! repaint rates a line per frame buries everything else. The first frame after a quiet spell is
//! owed a line at once, and frames landing inside an interval are held until that interval ends.
//!
//! Until it ends, not until the next frame, which is what the bridge used to do. The last frames
//! before a pause had no next frame, so they were never reported, and a pane that had echoed the
//! last keystroke typed into it was accused of having stopped painting (kan a_2PeXwg4fA).
//!
//! Pure: the clock arrives as nanoseconds, so the rule is tested without a thread or a sleep.

/// How much was painted since the last line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counted {
    pub(crate) frames: u64,
    pub(crate) bytes: u64,
}

#[derive(Debug)]
pub(crate) struct Tally {
    interval: u64,
    counted: Counted,
    /// When the last line went out, or `None` before the first.
    sent: Option<u64>,
}

impl Tally {
    pub(crate) const fn new(interval: u64) -> Tally {
        Tally { interval, counted: Counted { frames: 0, bytes: 0 }, sent: None }
    }

    /// A frame arrived.
    ///
    /// Answers whether it is the first counted since the last line, which is the only moment
    /// whatever waits on this tally needs waking: after that it is already waiting for the
    /// interval to end.
    pub(crate) fn count(&mut self, bytes: usize) -> bool {
        let first = self.counted.frames == 0;
        self.counted.frames += 1;
        self.counted.bytes += bytes as u64;
        first
    }

    /// How long until a line is owed, in the caller's units. Zero means now.
    ///
    /// `None` while nothing is counted, so a quiet pane costs no wakeups at all.
    pub(crate) fn due_in(&self, now: u64) -> Option<u64> {
        if self.counted.frames == 0 {
            return None;
        }
        let Some(sent) = self.sent else { return Some(0) };
        Some(self.interval.saturating_sub(now.saturating_sub(sent)))
    }

    /// What the line owed now should say, starting the next interval. `None` when none is owed.
    pub(crate) fn take(&mut self, now: u64) -> Option<Counted> {
        if self.due_in(now) != Some(0) {
            return None;
        }
        let counted = self.counted;
        self.counted = Counted { frames: 0, bytes: 0 };
        self.sent = Some(now);
        Some(counted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: u64 = 250;

    #[test]
    fn a_quiet_tally_owes_nothing() {
        let mut tally = Tally::new(INTERVAL);
        assert_eq!(tally.due_in(0), None);
        assert_eq!(tally.take(1_000_000), None);
    }

    #[test]
    fn the_first_frame_is_owed_a_line_at_once() {
        let mut tally = Tally::new(INTERVAL);
        assert!(tally.count(12), "the first frame is the one that wakes the reporter");
        assert_eq!(tally.due_in(0), Some(0));
        assert_eq!(tally.take(0), Some(Counted { frames: 1, bytes: 12 }));
        assert_eq!(tally.due_in(0), None, "and once taken nothing more is owed");
    }

    /// The bug this module exists for. A frame inside the interval used to wait for another
    /// frame to carry it, and the last frame before a pause never got one.
    #[test]
    fn a_frame_inside_the_interval_is_owed_a_line_when_the_interval_ends_with_no_frame_after_it() {
        let mut tally = Tally::new(INTERVAL);
        tally.count(12);
        tally.take(0);

        assert!(tally.count(5), "the first frame since a line wakes the reporter");
        assert!(!tally.count(7), "a second one finds it already waiting");
        assert_eq!(tally.due_in(10), Some(240));
        assert_eq!(tally.take(10), None, "held for the rest of the interval");
        assert_eq!(tally.take(250), Some(Counted { frames: 2, bytes: 12 }));
    }

    #[test]
    fn a_frame_after_a_quiet_spell_is_owed_a_line_at_once() {
        let mut tally = Tally::new(INTERVAL);
        tally.count(12);
        tally.take(0);

        tally.count(3);
        assert_eq!(tally.due_in(10_000), Some(0));
        assert_eq!(tally.take(10_000), Some(Counted { frames: 1, bytes: 3 }));
    }
}
