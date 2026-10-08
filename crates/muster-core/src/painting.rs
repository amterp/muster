//! Panes that were asked for something and have painted nothing since.
//!
//! Every way a pane goes silent leaves the same picture and nothing to read it by: the client stays connected, the bridge is
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
//! thing that knows the second, because output goes from its stdout into a surface and never past
//! the app. Both arrive here as facts and the rule is here.
//!
//! Two guards, each because the sentence would be false without it.
//!
//! **Only while the window is drawing the pane**, which is [`crate::typeable`]'s guard and holds
//! for the same reason: a pane nothing is drawing paints nothing, and its bridge belongs to a
//! surface that has been thrown away.
//!
//! That guard is about saying something new. A warning already raised stays raised when its pane
//! leaves the screen, because looking away does not make it untrue: a wedged bridge used to lose
//! its warning to a tab switch and stay frozen for three more minutes with nobody told (kan
//! a_2LWqtPd8E). It goes when something answers it - the pane painting, closing, being hidden,
//! or its machine going away - so a warning that went away says which of those happened.
//!
//! **Only while the pane's daemon is answering.** A machine that has gone away already raises one
//! problem naming itself; accusing each of its eight panes as well is the nagging that keying a
//! problem by its condition exists to end.
//!
//! **Naming the program when its daemon says it stopped reading.** The one cause the window can be
//! told rather than guess: the pane's daemon measured input its program has left unread. Then the
//! pane and its bridge are fine, and a Reattach would change nothing, so the warning says which
//! program, and offers none (GitHub issue #1). A warning raised before the daemon said so is said
//! again in those words once it does, and in the bridge's once the program reads.
//!
//! Pure - no clock, no sockets, no processes. Time arrives as a number, so every rule here is
//! driven by a recorded case.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::{DaemonId, PaneKey};
use crate::diagnostics::clock::describe;
use crate::problems::Remedy;
use crate::respawn::reattach_command;

/// What the problem list should be told, having compared the unanswered panes against the clock.
///
/// A diff, on the same terms as `typeable::Reported` and for the same reason:
/// the caller's job is to raise and clear, and a pane that stays silent is not news twice. The
/// watch asks repeatedly about a condition that stays true, so a reading that reported every
/// silent pane every time would republish the roster for as long as one stayed quiet.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reported {
    /// Panes that have just gone quiet for too long: the problem key, the whole sentence, and a
    /// reattach to offer beside it.
    pub raise: Vec<(String, String, Option<Remedy>)>,

    /// Keys that were raised and are no longer true, and why each stopped being true.
    ///
    /// The why is the difference between a pane that painted and a pane nobody could see any
    /// more, which cleared identically and had to be told apart by cross-referencing a bridge's
    /// byte counts against the daemon's own log (kan a_2LWqtPd8E).
    pub clear: Vec<(String, Cleared)>,
}

/// Why a problem this watch raised was taken back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleared {
    /// The pane painted.
    Painted,

    /// The pane is gone.
    Closed,

    /// The pane was hidden, so no window draws it and it has no bridge to paint through.
    Hidden,

    /// The pane's daemon stopped answering, and the machine's own problem says so instead.
    DaemonAway,

    /// The watch was switched off, so nothing it said stands.
    Off,
}

impl Cleared {
    /// The word for the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Cleared::Painted => "painted",
            Cleared::Closed => "closed",
            Cleared::Hidden => "hidden",
            Cleared::DaemonAway => "daemon_away",
            Cleared::Off => "off",
        }
    }
}

/// A pane's program that has stopped reading its input, as its daemon named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub program: String,
    pub pid: i32,
    /// What it had left unread when its daemon called it stopped.
    pub bytes: u32,
}

/// Every pane that owes output, and everything that decides whether owing it is worth saying.
#[derive(Debug, Default)]
pub struct Painting {
    /// When the earliest intent this pane has not painted since reached it.
    ///
    /// The earliest, not the latest, which is the whole of what this map holds that a single
    /// timestamp would not. Somebody who types into a pane that has frozen goes on typing, and a
    /// clock restarted by each keystroke is a deadline nobody ever reaches.
    asked: BTreeMap<PaneKey, u64>,

    /// Which panes have already been reported, so that clearing knows what to take back, and
    /// the program each was reported as having stopped reading, if it was.
    ///
    /// Held here rather than by the caller because it is the other half of the same rule: a pane
    /// is reported once and cleared once, and splitting the two across a lock boundary is how a
    /// stale error outlives the pane it was about. The cause is kept so that a pane whose cause
    /// became known, or went, is said again rather than left in the words of the old one.
    reported: BTreeMap<PaneKey, Option<Reader>>,

    /// Panes whose daemon says their program has stopped reading its input.
    not_reading: BTreeMap<PaneKey, Reader>,

    /// Why a reported pane stopped owing output since the last reading, for the clear that
    /// takes its problem back. Written wherever `asked` loses a pane, because that is the one
    /// thing a reading cannot see afterwards.
    settled: BTreeMap<PaneKey, Cleared>,

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
            reported: BTreeMap::new(),
            not_reading: BTreeMap::new(),
            settled: BTreeMap::new(),
            away: BTreeSet::new(),
            visible: None,
        }
    }

    /// An intent reached this pane, so output is owed.
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
    /// No timestamp, because there is nothing to compare one against: output arriving is the
    /// answer to whatever was outstanding, and the two facts reach this in the order they
    /// happened.
    pub fn painted(&mut self, pane: &PaneKey) -> bool {
        let owed = self.asked.remove(pane).is_some();
        if owed {
            self.settle(pane, Cleared::Painted);
        }
        owed
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
    ///
    /// Unless it has already been reported. That accusation was made while somebody was looking,
    /// and it stays until the pane paints: a warning that cleared on a tab switch read exactly
    /// like a pane that had painted, about a bridge that was still wedged.
    pub fn showing(&mut self, visible: BTreeSet<PaneKey>) {
        self.asked.retain(|pane, _| visible.contains(pane) || self.reported.contains_key(pane));
        self.visible = Some(visible);
    }

    /// Whether this daemon is answering, as the window's own health of it says.
    ///
    /// A daemon that has gone stale takes its panes' output with it, and says so once, naming the
    /// machine. What it leaves behind is what this drops: panes owed output that nothing on that
    /// machine is in a position to send.
    pub fn daemon_away(&mut self, daemon: &DaemonId, away: bool) {
        if away {
            self.away.insert(daemon.clone());
            let stranded: Vec<PaneKey> =
                self.asked.keys().filter(|pane| pane.daemon == *daemon).cloned().collect();
            for pane in &stranded {
                self.asked.remove(pane);
                self.settle(pane, Cleared::DaemonAway);
            }
        } else {
            self.away.remove(daemon);
        }
    }

    /// Whether the pane's program has stopped reading its input, as its daemon says: the reader
    /// that stopped, or `None` once it reads again or its daemon cannot say.
    ///
    /// Answers whether a reported pane's warning now owes different words, so a caller holding a
    /// clock knows to wake for it.
    pub fn stopped_reading(&mut self, pane: &PaneKey, reader: Option<Reader>) -> bool {
        let before = match reader {
            Some(reader) => self.not_reading.insert(pane.clone(), reader),
            None => self.not_reading.remove(pane),
        };
        self.reported.contains_key(pane) && before.as_ref() != self.not_reading.get(pane)
    }

    /// The pane is gone, so nothing is owed about it.
    ///
    /// Nothing is returned, and the problem is taken back by the next [`Painting::reconcile`]:
    /// the pane is no longer overdue and is still in `reported`, which is exactly the shape that
    /// method exists to answer. That keeps every call site here a recording and nothing more,
    /// which is what lets a caller hold this while holding whatever it was already holding.
    pub fn closed(&mut self, pane: &PaneKey) {
        self.asked.remove(pane);
        self.not_reading.remove(pane);
        self.settle(pane, Cleared::Closed);
    }

    /// The pane went into a tab no window shows, so nothing is owed about it until it is shown.
    ///
    /// Unlike leaving the screen, which keeps a warning raised: a hidden pane has no bridge, so it
    /// never paints, and a warning kept for it would stand until it closed - offering a Reattach
    /// that has nothing to reattach. Answers whether that changed anything, so a caller told on
    /// every reconcile wakes the clock only when it did.
    pub fn hidden(&mut self, pane: &PaneKey) -> bool {
        let owed = self.asked.remove(pane).is_some();
        self.settle(pane, Cleared::Hidden);
        owed
    }

    /// Compares the unanswered panes against the clock and says what the problem list owes.
    ///
    /// `deadline` is how long a pane may owe output, in the caller's own units. Zero switches
    /// this off, which is the honest answer for a run with no output to wait for.
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

        let causes: BTreeMap<PaneKey, Option<Reader>> = overdue
            .into_iter()
            .map(|pane| {
                let cause = self.not_reading.get(&pane).cloned();
                (pane, cause)
            })
            .collect();
        let reported = Reported {
            raise: causes
                .iter()
                // New, or said before in the words of another cause.
                .filter(|(pane, cause)| self.reported.get(*pane) != Some(*cause))
                .map(|(pane, cause)| match cause {
                    // Nothing to offer: the pane and its bridge are fine, and only something
                    // done to the program frees it.
                    Some(reader) => (key(pane), not_reading(pane, deadline, reader), None),
                    // Always safe to offer: the pane is one this window is drawing, so the
                    // bridge a reattach replaces is this window's own, and the agent behind it is
                    // untouched.
                    None => {
                        (key(pane), stopped(pane, deadline), Some(Remedy::Reattach(pane.clone())))
                    }
                })
                .collect(),
            clear: self
                .reported
                .keys()
                .filter(|pane| !causes.contains_key(*pane))
                .map(|pane| {
                    // Every other way out of `overdue` settles first, so this is a deadline of
                    // zero.
                    let why = self.settled.get(pane).copied().unwrap_or(Cleared::Off);
                    (key(pane), why)
                })
                .collect(),
        };
        self.settled.clear();
        self.reported = causes;
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
                    let said = self.reported.get(pane);
                    // Owed a reading too when its warning's cause has changed since it was said.
                    (said.is_none_or(|cause| cause.as_ref() != self.not_reading.get(pane)))
                        .then_some(0)
                }
            })
            .min()
    }

    /// Records why a reported pane stopped owing output. A pane nobody was told about has
    /// nothing to take back, so nothing is kept for it.
    fn settle(&mut self, pane: &PaneKey, why: Cleared) {
        if self.reported.contains_key(pane) {
            self.settled.insert(pane.clone(), why);
        }
    }

    /// Whether this pane's silence would be worth saying anything about.
    fn counts(&self, pane: &PaneKey) -> bool {
        let drawn = self.visible.as_ref().is_none_or(|visible| visible.contains(pane));
        (drawn || self.reported.contains_key(pane)) && !self.away.contains(&pane.daemon)
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
/// It cannot name the cause - that is what a wide net costs. What it can do is say which layer is
/// implicated, where the records that separate the causes are, and what puts the pane back
/// without touching the agent behind it.
///
/// The remedy is a reattach, because it is the way to a new bridge that keeps the agent. Closing
/// the pane gets a new bridge too, by ending what is running in it, and a sentence advising that
/// costs the reader the session they were trying to rescue (kan a_2MjBI7BLr).
///
/// The last clause is the one that saves somebody an hour. A pane that is genuinely fine says
/// this too, if the program in it turned echo off and is waiting for a password, and a reader who
/// does not know that goes looking for a broken bridge that was never broken.
pub fn stopped(pane: &PaneKey, deadline: u64) -> String {
    let waited = describe(deadline);
    let reattach = reattach_command(&pane.pane);
    format!(
        "Input reached the pane {pane} over {waited} ago and it has painted nothing since. It \
         shows whatever it painted last, so it reads as a program that has stopped rather than a \
         pane that has - the agent behind it goes on working and every other pane in the window \
         is unaffected. Usual causes: the bridge carrying this pane's output has wedged or gone \
         without saying so, or its daemon has stopped sending this one pane's output while it \
         goes on answering everything else. The run log carries `bridge.painted` for as long as \
         output arrives, and `bridge.ended` or `link.bridge.gone` when a bridge ends; output \
         the daemon did not send is explained in the daemon's own log on its own machine. \
         {reattach} starts a new bridge and leaves the agent alone. A pane that is fine says \
         this too if what is running in it turned echo off - a password prompt - and takes it \
         back the moment anything paints."
    )
}

/// What to tell somebody whose pane has painted nothing because its program stopped reading.
///
/// The one cause the window is told rather than left to guess, so the sentence says it and
/// clears the bridge and the daemon, which [`stopped`] has to implicate. It offers no reattach:
/// a new bridge would paint the same frozen screen. What frees the pane is done to its program,
/// and only the person knows whether interrupting it costs them anything.
pub fn not_reading(pane: &PaneKey, deadline: u64, reader: &Reader) -> String {
    let waited = describe(deadline);
    let Reader { program, pid, bytes } = reader;
    let bytes = if *bytes == 1 { "1 byte".to_string() } else { format!("{bytes} bytes") };
    format!(
        "Input reached the pane {pane} over {waited} ago and nothing has painted since, because \
         its program, {program} (pid {pid}), has stopped reading its input: it has left {bytes} \
         unread. The pane, its bridge and its daemon are fine, so a reattach would change \
         nothing. Interrupting the program, or ending it and resuming its session, frees it; \
         this goes the moment it reads again. Its daemon's log carries `daemon.pane.unread_input` \
         from when it stopped."
    )
}
