//! Panes waiting for a bridge, and which of them have waited too long.
//!
//! A pane becomes typeable when its bridge dials the socket Muster bound for it. Until then
//! it renders, paints, and discards every keystroke - and three separate bugs in this repo's
//! history all ended in exactly that state: the bridge failed to dial, the socket path had
//! moved, the channel could not be opened. One symptom, three causes, and nothing said so
//! until somebody typed.
//!
//! What makes it reportable is that both ends of the wait are already known. The core binds
//! the socket, so it knows when the wait started, and it runs the callback the accept fires,
//! so it knows when the wait ended. The only thing missing was a deadline between them.
//!
//! Pure - no clock, no thread, no socket. Time arrives as a number, so every rule here is
//! driven by a recorded case: whose deadline has passed, what to say about it, and what to
//! take back when a bridge turns up late.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::PaneKey;

/// What the problem list should be told, having compared the waiting panes against the clock.
///
/// A diff on both halves, because the caller's job is to raise and clear and those are the two
/// things it can do. Working it out here is what keeps the thread holding the clock down to
/// three lines, and it puts the rule that matters under a case: the watch asks repeatedly about
/// a condition that stays true, so a reading that reported every overdue pane every time would
/// republish the roster for as long as a pane stayed quiet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reported {
    /// Panes that have just fallen overdue: the problem key and the whole sentence to say.
    pub raise: Vec<(String, String)>,

    /// Keys that were raised and are no longer true.
    pub clear: Vec<String>,
}

/// Every pane whose socket is bound and whose bridge has not dialed.
#[derive(Debug, Default)]
pub struct Waiting {
    /// When each pane's wait started, on whatever monotonic scale the caller counts in.
    waiting: BTreeMap<PaneKey, u64>,

    /// Which panes have already been reported, so that clearing knows what to take back.
    ///
    /// Held here rather than by the caller because it is the other half of the same rule: a
    /// pane is reported once and cleared once, and splitting the two across a lock boundary
    /// is how a stale error outlives the pane it was about.
    reported: BTreeSet<PaneKey>,
}

impl Waiting {
    pub const fn new() -> Waiting {
        Waiting { waiting: BTreeMap::new(), reported: BTreeSet::new() }
    }

    /// A pane's socket is bound and its bridge is expected.
    ///
    /// Also how a wait restarts. A pane keeps its channel while its surface is thrown away
    /// and built again, so a bridge that exited is a bridge whose replacement has to dial
    /// too - and that second wait is the one `control_socket.rs` calls out as the exact
    /// failure the accept loop exists to prevent.
    pub fn opened(&mut self, pane: PaneKey, at: u64) {
        self.waiting.insert(pane, at);
    }

    /// A bridge dialed in, so this pane can be typed into.
    pub fn typeable(&mut self, pane: &PaneKey) {
        self.waiting.remove(pane);
    }

    /// The pane is gone, so nothing is owed about it.
    ///
    /// Separate from [`Waiting::typeable`] even though both stop the wait, because a closed
    /// pane is the case that goes wrong when it is forgotten: its error would otherwise
    /// outlive it and sit in the roster naming a pane nobody can look at.
    pub fn closed(&mut self, pane: &PaneKey) {
        self.waiting.remove(pane);
    }

    /// Compares the waiting panes against the clock and says what the problem list owes.
    ///
    /// `deadline` is how long a pane may wait, in the caller's own units. Zero switches this
    /// off, which is the honest answer for a run that has no bridges to wait for.
    pub fn reconcile(&mut self, now: u64, deadline: u64) -> Reported {
        let overdue: BTreeSet<PaneKey> = if deadline == 0 {
            BTreeSet::new()
        } else {
            self.waiting
                .iter()
                .filter(|(_, started)| now.saturating_sub(**started) >= deadline)
                .map(|(pane, _)| pane.clone())
                .collect()
        };

        let reported = Reported {
            raise: overdue
                .difference(&self.reported)
                .map(|pane| (key(pane), detail(pane, deadline)))
                .collect(),
            clear: self.reported.difference(&overdue).map(key).collect(),
        };
        self.reported = overdue;
        reported
    }

    /// How long until there is something new to say, or `None` when nothing more will change.
    ///
    /// So that a caller holding a real clock sleeps exactly as long as it has to, and an idle
    /// window costs no wakeups at all.
    ///
    /// Two answers are worth stating because they are the two ways a loop around this goes
    /// wrong. An overdue pane that has *already* been reported is not counted, or the answer
    /// would be zero forever and the loop would spin. An overdue pane that has *not* been
    /// reported answers zero, because a pane that fell overdue while the caller was busy
    /// elsewhere must not be slept through - and on a quiet window nothing else would ever
    /// wake it.
    pub fn next_wake(&self, now: u64, deadline: u64) -> Option<u64> {
        if deadline == 0 {
            return None;
        }
        self.waiting
            .iter()
            .filter_map(|(pane, started)| {
                let waited = now.saturating_sub(*started);
                if waited < deadline {
                    Some(deadline - waited)
                } else {
                    (!self.reported.contains(pane)).then_some(0)
                }
            })
            .min()
    }
}

/// Names the condition: this one pane cannot be typed into.
///
/// One key per pane rather than one for the window, because that is the shape of the
/// condition - fourteen panes working and one deaf is the case worth reporting precisely, and
/// a window-wide answer would either accuse the working panes or say nothing while one of
/// them swallowed everything typed into it.
///
/// Public because two other things have to spell it the same way: the corpus, which names the
/// keys it expects, and the seam's own test, which reads them back off the wire.
pub fn key(pane: &PaneKey) -> String {
    format!("pane:{pane}")
}

/// What to tell somebody whose pane is deaf, and what to do about it.
///
/// The deadline rather than how long it has actually been. An elapsed count would differ on
/// every reading, every reading would count as news to `Problems::raise`, and the roster
/// would republish and reopen itself for as long as the pane stayed quiet - which is exactly
/// the nagging that keying a problem by its condition was meant to end.
fn detail(pane: &PaneKey, deadline: u64) -> String {
    format!(
        "The pane {pane} has had a socket open for over {}, and nothing has dialed it - the \
         bridge carrying this pane's keystrokes either never started or cannot reach the \
         socket. Everything typed into this pane is discarded and it goes on rendering, so it \
         looks frozen rather than broken; every other pane in the window is unaffected. The \
         run log has the cause: look for `channel.accept.failed`, `bridge.exited.reported` \
         and `pane.channel.unavailable`. Closing this pane and opening it again starts a new \
         bridge.",
        describe(deadline),
    )
}

/// A duration in nanoseconds, as somebody reading a sentence would say it.
///
/// Whole units only, and no unit smaller than a millisecond: this appears in one sentence
/// about a pane that has stopped answering, where "5s" is the whole of what a reader needs
/// and "5.002s" would be precision about the wrong thing.
fn describe(nanos: u64) -> String {
    let millis = nanos / 1_000_000;
    if millis.is_multiple_of(1000) { format!("{}s", millis / 1000) } else { format!("{millis}ms") }
}
