//! Panes that were asked for something and have painted nothing since.
//!
//! The wider net under [`crate::grid`]. That one names a single cause of a pane that has stopped
//! updating - a frame too big for the daemon to send - and can say exactly what to do about it,
//! because Muster asked for the grid and knows the number. Every other way a pane goes silent
//! leaves the same picture and nothing to read it by: the client stays connected, the bridge is
//! still a process, the agent goes on working, and `muster window` reports the pane `idle`. A
//! wedged bridge, a transport that dropped without closing, a daemon still answering requests
//! while one of its terminals stopped painting - all of them are sixteen minutes of a frozen pane
//! whose only evidence is somewhere else (kan a_2LMRCug0P).
//!
//! **Absolute silence is not the condition.** An idle agent paints nothing all afternoon and is
//! perfectly healthy, so a watch on quiet alone would accuse most of a window most of the time.
//! What makes silence wrong is that something was asked of the pane: an intent that actually
//! reached it - a keystroke, a paste, a scroll - and nothing painted afterwards. The app is the
//! only thing that knows the first half, because it is the sender, and the bridge is the only
//! thing that knows the second, because frames go from its stdout into a surface and never past
//! the app. Both arrive here as facts and the rule is here.
//!
//! Three guards, each because the sentence would be false without it.
//!
//! **Only while the window is drawing the pane**, which is [`crate::typeable`]'s guard and holds
//! for the same reason: a pane nothing is drawing paints nothing, and its bridge belongs to a
//! surface that has been thrown away.
//!
//! **Only while the pane's daemon is answering.** A machine that has gone away already raises one
//! problem naming itself; accusing each of its eight panes as well is the nagging that keying a
//! problem by its condition exists to end.
//!
//! **Not when something else has already named this pane's silence.** A pane over the grid ceiling
//! is silent for a reason with a remedy in it - make the text bigger - and a second sentence
//! beside it saying only that the pane stopped would send the reader away from the answer they
//! already had.
//!
//! Pure - no clock, no sockets, no processes. Time arrives as a number, so every rule here is
//! driven by a recorded case.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::{DaemonId, PaneKey};
use crate::diagnostics::clock::describe;

/// What the problem list should be told, having compared the unanswered panes against the clock.
///
/// A diff, on the same terms as `typeable::Reported` and `grid::Reported` and for the same reason:
/// the caller's job is to raise and clear, and a pane that stays silent is not news twice. The
/// watch asks repeatedly about a condition that stays true, so a reading that reported every
/// silent pane every time would republish the roster for as long as one stayed quiet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reported {
    /// Panes that have just gone quiet for too long: the problem key and the whole sentence.
    pub raise: Vec<(String, String)>,

    /// Keys that were raised and are no longer true.
    pub clear: Vec<String>,
}

/// Every pane that owes a frame, and everything that decides whether owing one is worth saying.
#[derive(Debug, Default)]
pub struct Painting {
    /// When the earliest intent this pane has not painted since reached it.
    ///
    /// The earliest, not the latest, which is the whole of what this map holds that a single
    /// timestamp would not. Somebody who types into a pane that has frozen goes on typing, and a
    /// clock restarted by each keystroke is a deadline nobody ever reaches.
    asked: BTreeMap<PaneKey, u64>,

    /// Which panes have already been reported, so that clearing knows what to take back.
    ///
    /// Held here rather than by the caller because it is the other half of the same rule: a pane
    /// is reported once and cleared once, and splitting the two across a lock boundary is how a
    /// stale error outlives the pane it was about.
    reported: BTreeSet<PaneKey>,

    /// Panes whose silence something else has already explained.
    explained: BTreeSet<PaneKey>,

    /// Daemons that have stopped answering, whose panes cannot paint and are not at fault.
    away: BTreeSet<DaemonId>,

    /// Which panes the window is drawing, or `None` while nothing has said.
    ///
    /// `None` is not "no panes" - it is the state before a window has published anything, and
    /// filtering on it would be filtering on ignorance. So it means every pane counts, which is
    /// also the only safe direction to be wrong in here.
    visible: Option<BTreeSet<PaneKey>>,
}

impl Painting {
    pub const fn new() -> Painting {
        Painting {
            asked: BTreeMap::new(),
            reported: BTreeSet::new(),
            explained: BTreeSet::new(),
            away: BTreeSet::new(),
            visible: None,
        }
    }

    /// An intent reached this pane, so a frame is owed.
    ///
    /// Delivered rather than merely sent: input that went nowhere is a pane that cannot be typed
    /// into, which is `typeable`'s condition and has its own sentence. Accusing a pane of not
    /// painting when nothing ever reached it would name the wrong half of the path.
    ///
    /// Answers whether this changed what the clock owes, so a caller holding one does not have to
    /// wake a thread for a pane that was already waiting.
    pub fn typed(&mut self, pane: &PaneKey, at: u64) -> bool {
        if self.asked.contains_key(pane) {
            return false;
        }
        self.asked.insert(pane.clone(), at);
        true
    }

    /// The pane painted, so it owes nothing.
    ///
    /// No timestamp, because there is nothing to compare one against: a frame arriving is the
    /// answer to whatever was outstanding, and the two facts reach this in the order they
    /// happened.
    pub fn painted(&mut self, pane: &PaneKey) -> bool {
        self.asked.remove(pane).is_some()
    }

    /// Which panes the window is drawing, as the view answered it.
    ///
    /// The same `View::showing` the roster, attention and the typeable watch are already settled
    /// against, so this is that answer reaching one more reader rather than a second opinion.
    ///
    /// A pane that leaves the screen drops what it owed. Nothing can paint it and nothing can be
    /// typed into it, so carrying the reading forward would accuse it for a silence that started
    /// the moment its surface was thrown away - and getting this wrong in the loud direction is
    /// what makes a watch worth switching off.
    pub fn showing(&mut self, visible: BTreeSet<PaneKey>) {
        self.asked.retain(|pane, _| visible.contains(pane));
        self.visible = Some(visible);
    }

    /// Whether this daemon is answering, as the window's own health of it says.
    ///
    /// A daemon that has gone stale takes its panes' frames with it, and says so once, naming the
    /// machine. What it leaves behind is what this drops: panes that owe a frame nothing on that
    /// machine is in a position to send.
    pub fn daemon_away(&mut self, daemon: &DaemonId, away: bool) {
        if away {
            self.away.insert(daemon.clone());
            self.asked.retain(|pane, _| pane.daemon != *daemon);
        } else {
            self.away.remove(daemon);
        }
    }

    /// Whether something else has already said why this pane is silent.
    ///
    /// The grid ceiling is the one that does today: a pane too big to draw stops painting for a
    /// reason that carries its own remedy, and this net saying "and it stopped painting" beside
    /// it would be a second row sending the reader away from the answer.
    pub fn explained(&mut self, pane: &PaneKey, explained: bool) {
        if explained {
            self.explained.insert(pane.clone());
        } else {
            self.explained.remove(pane);
        }
    }

    /// The pane is gone, so nothing is owed about it.
    ///
    /// Nothing is returned, and the problem is taken back by the next [`Painting::reconcile`]:
    /// the pane is no longer overdue and is still in `reported`, which is exactly the shape that
    /// method exists to answer. That keeps every call site here a recording and nothing more,
    /// which is what lets a caller hold this while holding whatever it was already holding.
    pub fn closed(&mut self, pane: &PaneKey) {
        self.asked.remove(pane);
        self.explained.remove(pane);
    }

    /// Compares the unanswered panes against the clock and says what the problem list owes.
    ///
    /// `deadline` is how long a pane may owe a frame, in the caller's own units. Zero switches
    /// this off, which is the honest answer for a run with no frames to wait for.
    pub fn reconcile(&mut self, now: u64, deadline: u64) -> Reported {
        let overdue: BTreeSet<PaneKey> = if deadline == 0 {
            BTreeSet::new()
        } else {
            self.asked
                .iter()
                .filter(|(pane, _)| self.counts(pane))
                .filter(|(_, asked)| now.saturating_sub(**asked) >= deadline)
                .map(|(pane, _)| pane.clone())
                .collect()
        };

        let reported = Reported {
            raise: overdue
                .difference(&self.reported)
                .map(|pane| (key(pane), stopped(pane, deadline)))
                .collect(),
            clear: self.reported.difference(&overdue).map(key).collect(),
        };
        self.reported = overdue;
        reported
    }

    /// How long until there is something new to say, or `None` when nothing more will change.
    ///
    /// So that a caller holding a real clock sleeps exactly as long as it has to, and a window
    /// whose panes are all answering costs no wakeups at all.
    ///
    /// A pane that is overdue and already reported is not counted, or the answer would be zero
    /// forever and the loop around this would spin. One that is overdue and not yet reported
    /// answers zero, because a pane that fell overdue while the caller was busy elsewhere must
    /// not be slept through.
    pub fn next_wake(&self, now: u64, deadline: u64) -> Option<u64> {
        if deadline == 0 {
            return None;
        }
        self.asked
            .iter()
            .filter(|(pane, _)| self.counts(pane))
            .filter_map(|(pane, asked)| {
                let waited = now.saturating_sub(*asked);
                if waited < deadline {
                    Some(deadline - waited)
                } else {
                    (!self.reported.contains(pane)).then_some(0)
                }
            })
            .min()
    }

    /// Whether this pane's silence would be worth saying anything about.
    fn counts(&self, pane: &PaneKey) -> bool {
        self.visible.as_ref().is_none_or(|visible| visible.contains(pane))
            && !self.away.contains(&pane.daemon)
            && !self.explained.contains(pane)
    }
}

/// Names the condition: this one pane has stopped painting.
///
/// One key per pane rather than one for the window, on the same terms as the two conditions
/// beside it: fourteen panes working and one frozen is the case worth reporting precisely.
///
/// Public because two other things spell it the same way - the corpus, which names the keys it
/// expects, and the bridge's own test, which reads them back off the wire.
pub fn key(pane: &PaneKey) -> String {
    format!("pane:{pane}:painting")
}

/// What to tell somebody whose pane has stopped painting, and what to do about it.
///
/// The deadline rather than how long it has actually been, for the reason `typeable::detail`
/// gives: an elapsed count differs on every reading, every reading would count as news, and the
/// roster would republish itself for as long as the pane stayed quiet.
///
/// It cannot name the cause, which is the difference between this and the sentence the grid
/// ceiling writes - that is what a wider net costs. What it can do is say which layer is
/// implicated, where the records that separate the causes are, and what puts the pane back
/// without touching the agent behind it.
///
/// The last clause is the one that saves somebody an hour. A pane that is genuinely fine says
/// this too, if the program in it turned echo off and is waiting for a password, and a reader who
/// does not know that goes looking for a broken bridge that was never broken.
pub fn stopped(pane: &PaneKey, deadline: u64) -> String {
    let waited = describe(deadline);
    format!(
        "Input reached the pane {pane} over {waited} ago and it has painted nothing since. It \
         shows whatever it painted last, so it reads as a program that has stopped rather than a \
         pane that has - the agent behind it goes on working and every other pane in the window \
         is unaffected. Usual causes: the bridge carrying this pane's frames has wedged or gone \
         without saying so, or its daemon has stopped painting this one terminal while it goes \
         on answering everything else. The run log carries `bridge.painted` for as long as \
         frames arrive, and `bridge.closed` or `channel.bridge.gone` when a bridge ends; a frame \
         the daemon decided not to send is in the daemon's own log on its own machine. Closing \
         this pane and opening it again starts a new bridge and leaves the agent alone. A pane \
         that is fine says this too if what is running in it turned echo off - a password prompt \
         - and takes it back the moment anything paints."
    )
}
