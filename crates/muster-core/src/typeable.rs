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
use crate::respawn::{self, Ended, Ending};

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

/// One pane's wait, and what is known about why it is waiting.
#[derive(Debug, Clone)]
struct Wait {
    /// When this wait started, on whatever monotonic scale the caller counts in.
    since: u64,

    /// How the last bridge for this pane ended, when there was one.
    ///
    /// The difference between a sentence somebody can act on and a sentence pointing at a log
    /// file. `None` is a pane whose first bridge has not arrived, which is the launch case and
    /// has nothing to explain beyond the wait itself.
    last: Option<Ended>,
}

/// Every pane whose socket is bound and whose bridge has not dialed.
#[derive(Debug, Default)]
pub struct Waiting {
    waiting: BTreeMap<PaneKey, Wait>,

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
        self.waiting.insert(pane, Wait { since: at, last: None });
    }

    /// A bridge for this pane has ended, so the wait starts again knowing why.
    ///
    /// Separate from [`Waiting::opened`] only in what it carries. A pane keeps its channel
    /// while its surface is thrown away and built again, so the wait restarting is the same
    /// wait either way - what is different is that this one can say what happened to the last
    /// bridge, and a pane that stays dark for five seconds after a refused attach has a remedy
    /// where a pane at launch has only a deadline.
    pub fn ended(&mut self, pane: PaneKey, at: u64, ended: Ended) {
        self.waiting.insert(pane, Wait { since: at, last: Some(ended) });
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
                .filter(|(_, wait)| now.saturating_sub(wait.since) >= deadline)
                .map(|(pane, _)| pane.clone())
                .collect()
        };

        let reported = Reported {
            raise: overdue
                .difference(&self.reported)
                .map(|pane| {
                    let last = self.waiting.get(pane).and_then(|wait| wait.last.as_ref());
                    (key(pane), detail(pane, deadline, last))
                })
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
            .filter_map(|(pane, wait)| {
                let waited = now.saturating_sub(wait.since);
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
///
/// Four sentences rather than one, because the pane looks identical in all four cases and the
/// thing to do differs in every one. Until this, every one of them read as "look in the run
/// log", which is a file nobody has open at the moment their pane stops answering - and the
/// run log itself had the impact and the remedy on the same line all along.
fn detail(pane: &PaneKey, deadline: u64, last: Option<&Ended>) -> String {
    let waited = describe(deadline);
    match last.map(|ended| ended.ending) {
        // Never had a bridge. The launch case, and the three bugs this watch was written for:
        // the bridge failed to dial, the socket path had moved, the channel could not be
        // opened. Nothing has said anything about this pane, so the log is the only lead.
        None => format!(
            "The pane {pane} has had a socket open for over {waited}, and nothing has dialed \
             it - the bridge carrying this pane's keystrokes either never started or cannot \
             reach the socket. Everything typed into this pane is discarded and it goes on \
             rendering, so it looks frozen rather than broken; every other pane in the window \
             is unaffected. The run log has the cause: look for `channel.accept.failed`, \
             `bridge.exited.reported` and `pane.channel.unavailable`. Closing this pane and \
             opening it again starts a new bridge."
        ),

        // Somebody else has it, and Muster left it to them on purpose.
        Some(Ending::TakenOver) => respawn::yielded(pane),

        // The one nobody guesses, and the one that cost a working day: a herdr client whose
        // transport died goes on holding the terminal, and every attach after that is refused
        // by a machine that is otherwise perfectly healthy.
        Some(Ending::Refused) => format!(
            "The pane {pane} has been dark for over {waited}: something else is holding its \
             terminal, and every bridge Muster started for it was refused. Only one client may \
             hold a herdr terminal, and one whose connection died goes on holding it without \
             noticing - most often a previous Muster's client, still on the far machine. The \
             agent behind this pane is untouched and every other pane in the window is \
             unaffected. {} releases it, and closing this pane and opening it again then starts \
             a bridge that attaches.",
            respawn::release_command(pane),
        ),

        // The connection went, Muster started another bridge, and that one has not dialed
        // either. Naming the host is the remedy: this is what a dropped VPN looks like from a
        // pane, and it recovers on its own the moment the machine is reachable again.
        Some(Ending::Lost) => format!(
            "The pane {pane} lost the connection carrying it and has not got one back within \
             {waited}. It shows what it last painted and takes no keystrokes; the agent behind \
             it is untouched, and panes on other machines in this window are unaffected. Check \
             that the machine holding it is reachable - the run log says `tunnel.down` when the \
             connection is the reason, and Muster keeps trying. Closing this pane and opening \
             it again starts a fresh bridge."
        ),
    }
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
