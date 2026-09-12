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
//! And a third thing, which the deadline alone got wrong: whether anybody is looking. The
//! accusation is that a pane renders and swallows what is typed into it, and a pane nothing is
//! drawing renders nothing - so a wait is only counted while the window is showing that pane,
//! and a pane that comes back waits again from the moment it is drawn.
//!
//! **What waits here also asks.** Saying so was the whole of this at first, and saying so was
//! not enough: a pane whose bridge was decided on and never started has nothing that can ask
//! for another, because everything that asks is driven by a bridge ending and this one never
//! began. Two agents sat unreachable for ninety minutes that way (kan a_2KIPfvt7L). The
//! condition was already computed here, exactly - socket bound, nothing dialed, the window
//! drawing it, the deadline passed - so this says which panes to ask for as well as which to
//! report, and `respawn` decides whether asking is the right answer for each.
//!
//! Pure - no clock, no thread, no socket. Time arrives as a number, so every rule here is
//! driven by a recorded case: whose deadline has passed, what to say about it, which to ask a
//! bridge for, and what to take back when one turns up late.

use std::collections::{BTreeMap, BTreeSet};

use crate::composition::PaneKey;
use crate::diagnostics::clock::describe;
use crate::respawn::{self, Ended, Ending};

/// How many deadlines a pane waits before a bridge is asked for, rather than only reported.
///
/// Three, and not one, because saying something and doing something cost different amounts of
/// being wrong. A sentence about a pane that turns out to be fine is withdrawn a moment later
/// and costs a row in a list. An ask tears down the surface a bridge is the command of - so an
/// ask aimed at a bridge that was merely slow kills one that was about to work, and replaces it
/// with one that is just as slow on a machine that is just as busy. That is a recovery that
/// prevents recovery, and a loaded machine is exactly the condition this was written for.
///
/// The deadline is already 2.5x the budget a launch known to work fits inside. Three of them is
/// past anything measured here, so a bridge that has not dialed by then is absent rather than
/// late. Expressed as a multiple so that a run which shortens the deadline shortens both.
pub const ASK_AFTER_DEADLINES: u64 = 3;

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

    /// Panes that should be asked for a bridge, because nothing has dialed them.
    ///
    /// Every overdue pane whose last ask is [`ASK_AFTER_DEADLINES`] deadlines old, not only the
    /// ones that have just fallen overdue - which is the difference between this and `raise`.
    /// Saying a thing twice is nagging; asking twice is the recovery, because the first ask is
    /// exactly what may have produced nothing.
    pub stalled: Vec<PaneKey>,
}

/// One pane's wait, and what is known about why it is waiting.
#[derive(Debug, Clone)]
struct Wait {
    /// When this wait started, on whatever monotonic scale the caller counts in.
    since: u64,

    /// When a bridge for this pane was last asked for.
    ///
    /// Separate from `since` because the two answer different questions and must not move
    /// together. How long the pane has been deaf is what the sentence is about, and it has to
    /// keep climbing or a problem raised would clear itself and be raised again every
    /// deadline - the nagging that keying a problem by its condition exists to end. How long
    /// ago somebody asked is what paces the asking, and that has to restart on every ask or
    /// the pane would be asked for on every tick.
    asked: u64,

    /// How the last bridge for this pane ended, when there was one.
    ///
    /// The difference between a sentence somebody can act on and a sentence pointing at a log
    /// file. `None` is a pane whose first bridge has not arrived, which is the launch case and
    /// has nothing to explain beyond the wait itself.
    last: Option<Ended>,

    /// What the backend calls this pane, for the one sentence that is not about Muster.
    ///
    /// Carried rather than derived because this module has no registry and the difference is
    /// invisible from here: `PaneKey` spells the pane the way everything above the adapter
    /// does, and the remedy for a held terminal is matched against a herdr client's command
    /// line, which spells it the backend's way. Empty is a pane whose channel was never
    /// opened, and the sentence says to look the name up rather than naming the wrong one.
    backend: String,
}

/// Every pane whose socket is bound and whose bridge has not dialed.
#[derive(Debug, Default)]
pub struct Waiting {
    waits: BTreeMap<PaneKey, Wait>,

    /// Which panes have already been reported, so that clearing knows what to take back.
    ///
    /// Held here rather than by the caller because it is the other half of the same rule: a
    /// pane is reported once and cleared once, and splitting the two across a lock boundary
    /// is how a stale error outlives the pane it was about.
    reported: BTreeSet<PaneKey>,

    /// Which panes the window is drawing, or `None` while nothing has said.
    ///
    /// `None` is not "no panes". It is the state before a window has published anything, and
    /// filtering on it would be filtering on ignorance - so it means every waiting pane
    /// counts, which is also the only safe direction to be wrong in here.
    visible: Option<BTreeSet<PaneKey>>,
}

impl Waiting {
    pub const fn new() -> Waiting {
        Waiting { waits: BTreeMap::new(), reported: BTreeSet::new(), visible: None }
    }

    /// A pane's socket is bound and its bridge is expected.
    ///
    /// Also how a wait restarts. A pane keeps its channel while its surface is thrown away
    /// and built again, so a bridge that exited is a bridge whose replacement has to dial
    /// too - and that second wait is the one `control_socket.rs` calls out as the exact
    /// failure the accept loop exists to prevent.
    pub fn opened(&mut self, pane: PaneKey, at: u64, backend: String) {
        self.waits.insert(pane, Wait { since: at, asked: at, last: None, backend });
    }

    /// A bridge for this pane has ended, so the wait starts again knowing why.
    ///
    /// Separate from [`Waiting::opened`] only in what it carries. A pane keeps its channel
    /// while its surface is thrown away and built again, so the wait restarting is the same
    /// wait either way - what is different is that this one can say what happened to the last
    /// bridge, and a pane that stays dark for five seconds after a refused attach has a remedy
    /// where a pane at launch has only a deadline.
    ///
    /// Except when the daemon said the terminal no longer exists. No bridge can dial in for that
    /// pane, so there is nothing to wait for - and a wait started here accused the network, five
    /// seconds later, about a pane somebody had just closed (kan a_2LMpvavhA).
    pub fn ended(&mut self, pane: PaneKey, at: u64, ended: Ended) {
        if ended.ending == Ending::Gone {
            self.closed(&pane);
            return;
        }
        // The backend's name for the pane is carried over rather than asked for again: a
        // bridge ending is a bridge that had a channel, so the wait this replaces knows it.
        let backend = self.waits.get(&pane).map(|wait| wait.backend.clone()).unwrap_or_default();
        self.waits.insert(pane, Wait { since: at, asked: at, last: Some(ended), backend });
    }

    /// A bridge dialed in, so this pane can be typed into.
    pub fn typeable(&mut self, pane: &PaneKey) {
        self.waits.remove(pane);
    }

    /// The pane is gone, so nothing is owed about it.
    ///
    /// Separate from [`Waiting::typeable`] even though both stop the wait, because a closed
    /// pane is the case that goes wrong when it is forgotten: its error would otherwise
    /// outlive it and sit in the roster naming a pane nobody can look at.
    pub fn closed(&mut self, pane: &PaneKey) {
        self.waits.remove(pane);
    }

    /// Which panes the window is drawing, as the view answered it.
    ///
    /// What this raises is that a pane renders and discards everything typed into it. A pane
    /// nothing is drawing renders nothing, so the sentence is false about it however long its
    /// socket has been bound - and a watch that says it anyway is wrong on its own terms.
    /// That is the whole reason this exists; a zoomed tab is only where somebody noticed,
    /// three false alarms at a time on every launch onto one.
    ///
    /// The set is `View::showing`, which is the window's single answer to which panes are on
    /// screen and is what the roster and attention are already settled against. So this is
    /// that answer reaching one more reader rather than a second opinion about it.
    ///
    /// A pane that comes back waits again from here. Its clock ran while nobody could see it,
    /// so carrying that reading forward would accuse a bridge the moment its pane is drawn,
    /// for a silence nobody was in a position to notice - and a pane that is genuinely deaf
    /// still says so, one full deadline after it is drawn again. Getting this half wrong in
    /// the quiet direction would remove the alarm rather than correct it.
    pub fn showing(&mut self, visible: BTreeSet<PaneKey>, at: u64) {
        for (pane, wait) in &mut self.waits {
            let drawn = self.visible.as_ref().is_none_or(|held| held.contains(pane));
            if !drawn && visible.contains(pane) {
                wait.since = at;
                wait.asked = at;
            }
        }
        self.visible = Some(visible);
    }

    /// Compares the waiting panes against the clock and says what the problem list owes.
    ///
    /// `deadline` is how long a pane may wait, in the caller's own units. Zero switches this
    /// off, which is the honest answer for a run that has no bridges to wait for.
    pub fn reconcile(&mut self, now: u64, deadline: u64) -> Reported {
        let overdue: BTreeSet<PaneKey> = if deadline == 0 {
            BTreeSet::new()
        } else {
            self.waits
                .iter()
                .filter(|(pane, _)| self.drawn(pane))
                .filter(|(_, wait)| now.saturating_sub(wait.since) >= deadline)
                .map(|(pane, _)| pane.clone())
                .collect()
        };

        // Every overdue pane whose last ask is a deadline old, which is what paces the asking.
        // Taken before `asked` is restamped below, and separately from `raise`, because the
        // two are opposite rules on purpose: a condition that stays true is said once, and a
        // bridge that never arrived is asked for again.
        let ask_after = deadline.saturating_mul(ASK_AFTER_DEADLINES);
        let stalled: Vec<PaneKey> = overdue
            .iter()
            .filter(|pane| {
                self.waits
                    .get(*pane)
                    .is_some_and(|wait| now.saturating_sub(wait.asked) >= ask_after)
            })
            .cloned()
            .collect();

        let reported = Reported {
            raise: overdue
                .difference(&self.reported)
                .map(|pane| {
                    let wait = self.waits.get(pane);
                    let backend = wait.map_or("", |wait| wait.backend.as_str());
                    (key(pane), detail(pane, deadline, wait.and_then(|w| w.last.as_ref()), backend))
                })
                .collect(),
            clear: self.reported.difference(&overdue).map(key).collect(),
            stalled: stalled.clone(),
        };
        for pane in &stalled {
            if let Some(wait) = self.waits.get_mut(pane) {
                wait.asked = now;
            }
        }
        self.reported = overdue;
        reported
    }

    /// How long until there is something new to say, or `None` when nothing more will change.
    ///
    /// So that a caller holding a real clock sleeps exactly as long as it has to, and an idle
    /// window costs no wakeups at all.
    ///
    /// Two answers are worth stating because they are the two ways a loop around this goes
    /// wrong. An overdue pane that has *already* been reported is not counted for the saying
    /// of it, or the answer would be zero forever and the loop would spin. An overdue pane
    /// that has *not* been reported answers zero, because a pane that fell overdue while the
    /// caller was busy elsewhere must not be slept through - and on a quiet window nothing
    /// else would ever wake it.
    ///
    /// The asking answers separately, and is why a stalled pane does not simply go quiet. A
    /// pane nothing has dialed is asked for again a deadline after the last ask, whether or
    /// not anybody has been told about it - so the loop keeps waking for as long as the pane
    /// is dark, rather than sleeping forever the moment its problem is raised.
    pub fn next_wake(&self, now: u64, deadline: u64) -> Option<u64> {
        if deadline == 0 {
            return None;
        }
        self.waits
            .iter()
            .filter(|(pane, _)| self.drawn(pane))
            .map(|(pane, wait)| {
                let waited = now.saturating_sub(wait.since);
                let to_say = if waited < deadline {
                    Some(deadline - waited)
                } else {
                    (!self.reported.contains(pane)).then_some(0)
                };
                let to_ask = deadline
                    .saturating_mul(ASK_AFTER_DEADLINES)
                    .saturating_sub(now.saturating_sub(wait.asked));
                to_say.map_or(to_ask, |say| say.min(to_ask))
            })
            .min()
    }

    /// Whether the window is drawing this pane, and so whether anything is owed about it.
    fn drawn(&self, pane: &PaneKey) -> bool {
        self.visible.as_ref().is_none_or(|visible| visible.contains(pane))
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
fn detail(pane: &PaneKey, deadline: u64, last: Option<&Ended>, backend_pane: &str) -> String {
    let waited = describe(deadline);
    let asking = describe(deadline.saturating_mul(ASK_AFTER_DEADLINES));
    let reattach = respawn::reattach_command(&pane.pane);
    match last.map(|ended| ended.ending) {
        // Never had a bridge. The launch case, and the three bugs this watch was written for:
        // the bridge failed to dial, the socket path had moved, the channel could not be
        // opened. Nothing has said anything about this pane, so the log is the only lead.
        //
        // `Gone` never waits, because `Waiting::ended` drops it, so it shares the sentence that
        // claims the least.
        None | Some(Ending::Gone) => format!(
            "The pane {pane} has had a socket open for over {waited}, and nothing has dialed \
             it - the bridge carrying this pane's keystrokes either never started or cannot \
             reach the socket. Everything typed into this pane is discarded and it goes on \
             rendering, so it looks frozen rather than broken; every other pane in the window \
             is unaffected. Muster asks for another bridge every {asking} and keeps asking; \
             {reattach} asks now. The run log has the cause: look for \
             `channel.accept.failed`, `bridge.exited.reported` and `pane.channel.unavailable`."
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
             unaffected. {} releases it, and {reattach} then asks for a bridge that attaches.",
            respawn::release_command(backend_pane),
        ),

        // The connection went, Muster started another bridge, and that one has not dialed
        // either. Naming the host is the remedy: this is what a dropped VPN looks like from a
        // pane, and it recovers on its own the moment the machine is reachable again.
        Some(Ending::Lost) => format!(
            "The pane {pane} lost the connection carrying it and has not got one back within \
             {waited}. It shows what it last painted and takes no keystrokes; the agent behind \
             it is untouched, and panes on other machines in this window are unaffected. Check \
             that the machine holding it is reachable - the run log says `tunnel.down` when the \
             connection is the reason, and Muster keeps asking for a bridge until one dials. \
             {reattach} asks now, once the machine is back."
        ),
    }
}
