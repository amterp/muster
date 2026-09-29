//! Bridges that ended, and which of them are worth replacing.
//!
//! A pane's bridge dies for two reasons that look identical from here. The connection carrying
//! it went - a laptop swapping ethernet for wifi kills every ssh under it - and the pane on the
//! far machine is untouched, so another bridge renders it again. Or the bridge cannot do its
//! job at all, and every replacement will end the same way in the same fraction of a second.
//! Replacing the first is the whole of kan a_2HrmSyRAQ; replacing the second is a loop that
//! spawns processes until somebody quits the app.
//!
//! Whether the daemon still holds the pane is asked before this and is not the answer: after a
//! network change the daemon holds it and an attach still fails, until the connection to the far
//! machine is back. So the thing that
//! separates them is how long the last bridge lasted. One that ran for an hour and then died
//! is a connection; one that died on sight, three times inside half a minute, is not going to
//! work on the fourth try either.
//!
//! Dialing back cannot be the health signal, which is worth stating because it is the obvious
//! candidate. A bridge whose attach is refused still reaches the app first - it dials, then
//! asks the daemon, then reports the refusal and exits - so a rule that reset on a dial would
//! reset on exactly the failure it is meant to stop.
//!
//! **A bridge that never started is a third case, and for two releases nothing here could see
//! it.** Every rule above is driven by a bridge *ending*, and a replacement that was decided on
//! and never began has no exit to notice - so nothing counted one, nothing published a number,
//! no surface was built, and the pane had no bridge for the life of the app process with its
//! agent still running behind it (kan a_2KIPfvt7L). [`Respawns::stalled`] is that case, told to
//! this module by the watch that already knows which panes nothing is dialing.
//!
//! Pure - no clock, no processes. Time arrives as a number, so every rule here is driven by a
//! recorded case.

use std::collections::BTreeMap;

use crate::composition::PaneKey;
use crate::mirror::backend::PaneId;

/// How many replacements a pane gets before Muster stops and says so.
///
/// Three, because the case worth surviving is a network that comes back within a few seconds
/// and the case worth stopping is one that never will. Two would give up on a machine that
/// takes one extra moment to let go of a pane; ten would be forty seconds of spawning
/// processes at a pane nobody can rescue.
pub const LIMIT: u32 = 3;

/// How long a bridge must last before it counts as having worked, in nanoseconds.
///
/// Thirty seconds. Above the several hundred milliseconds a remote bridge needs to start,
/// attach and paint - the far machine alone takes about 400ms of that - and far below any
/// interval a person would call "it was fine and then it broke".
pub const SETTLED_NS: u64 = 30_000_000_000;

/// Why a bridge stopped, in Muster's words rather than the daemon's.
///
/// Four, because they are the four the app has to answer differently. The daemon says only
/// why a stream ended; `muster_core::bridge_link` is where that becomes one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The stream carrying it ended, and nothing said why. A route that changed, a daemon that
    /// restarted, a pane that closed with nobody connected to hear it - and from here they are
    /// one thing, because the answer to all three is to look again and start another bridge if
    /// the pane is still there.
    Lost,

    /// The attach was refused: something else is already drawing this pane.
    ///
    /// Ordinary after a relaunch. A stream whose transport died stays open on the far machine
    /// until its ssh notices, so the first attach of a fresh app is refused by a machine that is
    /// otherwise perfectly healthy (kan a_2I76eCrjw). A replacement takes the pane over.
    Refused,

    /// Another bridge took the pane over, and the daemon handed it the stream.
    ///
    /// The one ending that must not be answered by attaching again. Somebody asked for this
    /// pane somewhere else and got it; taking it back would be answered the same way, and two
    /// windows would trade one pane until both gave up.
    TakenOver,

    /// The daemon says the pane no longer exists, which is what a bridge hears when its pane is
    /// closed under it.
    ///
    /// Split from `Lost` because the daemon has answered the question `Lost` leaves open. Looking
    /// again is not enough: the window can still be drawing the pane when this arrives, so a
    /// replacement goes to a pane nothing can attach to, and its failure to dial was then
    /// reported as a machine that could not be reached (kan a_2LMpvavhA).
    Gone,
}

impl Ending {
    /// The word for the wire and the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::Lost => "lost",
            Ending::Refused => "refused",
            Ending::TakenOver => "taken_over",
            Ending::Gone => "gone",
        }
    }

    pub fn parse(word: &str) -> Option<Ending> {
        match word {
            "lost" => Some(Ending::Lost),
            "refused" => Some(Ending::Refused),
            "taken_over" => Some(Ending::TakenOver),
            "gone" => Some(Ending::Gone),
            _ => None,
        }
    }
}

/// A bridge that has stopped, and what it managed to say first.
///
/// Here rather than beside the socket it arrives on, because two things read it and neither is
/// that socket: this module decides whether to start another bridge, and `typeable` turns it
/// into the sentence the person reads when nothing does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub ending: Ending,

    /// The daemon's own sentence about it, when the bridge lived long enough to pass one on.
    ///
    /// Kept whole and untranslated, because it names the pane and the machine in the daemon's
    /// own words.
    pub reason: Option<String>,

    /// Whether that bridge ever painted anything.
    pub rendered: bool,
}

impl Ended {
    /// What a bridge that said nothing is taken to have meant.
    ///
    /// `Lost`, because that is the ending whose answer is to look again and start another, and
    /// a bridge that was killed - by a signal, by the machine going away - has said nothing and
    /// is exactly that case. A bridge refused its pane, told its pane has gone to somebody
    /// else, or told it no longer exists, has a moment to say so and does.
    pub fn unsaid() -> Ended {
        Ended { ending: Ending::Lost, reason: None, rendered: false }
    }
}

/// What to do about a bridge that has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Start another one. Carries which replacement this is, counting from one, which the view
    /// publishes so the shell knows to build a new surface and that this is a re-attach.
    Start(u32),

    /// Stop. Carries how many were tried, for the sentence the run log carries.
    GiveUp(u32),

    /// Leave this pane to whichever bridge now has it.
    ///
    /// Carries nothing and publishes nothing, deliberately. The count is what makes the shell
    /// build a new surface, so a yield that moved it would start the bridge it is refusing to
    /// start. What it does record is that this pane's bridge has ended, so that the watch on
    /// panes nothing is dialing does not ask for one either - a yielded pane is dark on
    /// purpose, and it looks from the outside exactly like a pane whose bridge never started.
    Yield,

    /// Start nothing, because the daemon says this pane no longer exists.
    ///
    /// Carries nothing, publishes nothing and records that the bridge ended, on the same terms
    /// as `Yield` and for the same reasons. A separate answer because the run log says which it
    /// was: that pane has somebody else holding it, and this one is gone.
    Leave,
}

/// Every pane whose bridge Muster has replaced, and how recently.
#[derive(Debug, Default)]
pub struct Respawns {
    started: BTreeMap<PaneKey, Started>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Started {
    /// How many bridges this pane has been asked for, ever. Only ever climbs.
    ///
    /// What the view publishes, and a different number from `tried` for a reason that cost
    /// this project a bug: the shell builds a new surface when this *changes*, so a number
    /// that comes back to one it has already seen is a pane that never gets its next bridge.
    /// A run of failures starting over is a fact about the limit, not about the surface.
    restarts: u32,

    /// How many replacements have been asked for in the current run of failures.
    ///
    /// What [`LIMIT`] is about, and what `Decision::GiveUp` carries.
    tried: u32,

    /// When the last one was asked for, on whatever monotonic scale the caller counts in.
    ///
    /// Stands in for how long that bridge lived, which is what the rule is really about: the
    /// shell starts one within a frame of being told to, so the gap between asking and the
    /// next exit is the bridge's life to within a few milliseconds.
    asked_at: u64,

    /// Whether this pane was left to another window, or to the daemon that says it is gone,
    /// rather than given up on. Only a person asks for a bridge for such a pane.
    left: bool,

    /// Whether the bridge asked for at `asked_at` has ended.
    ///
    /// The difference between the two failures that look identical from a pane: a bridge that
    /// ran and died, which [`Respawns::ended`] has already answered for, and a bridge that
    /// never started at all, which nothing has answered for because nothing ended. Only the
    /// second is [`Respawns::stalled`]'s to act on.
    ended: bool,
}

impl Respawns {
    pub const fn new() -> Respawns {
        Respawns { started: BTreeMap::new() }
    }

    /// A bridge for this pane has ended, and its daemon still holds the pane.
    ///
    /// Recorded either way. A pane that gave up keeps its count, so a later exit does not
    /// start the run of failures over from one - there is no later exit while nothing is
    /// running, and if something does start one it is a fresh bridge that has to earn its own
    /// place.
    ///
    /// `ending` decides one thing and only one: whether attaching again is the right answer at
    /// all. For two of the four it is - a connection that went and a pane held by a bridge that
    /// has not noticed its transport died are both recovered by attaching again, and the second
    /// needs the `--takeover` a replacement carries. For the other two it is not. A pane handed
    /// to another bridge was handed to somebody who asked for it, and taking it back would be
    /// answered the same way from the other side: two windows trading one pane at the speed a
    /// bridge starts, until both of them ran out of tries. And a pane the daemon says no longer
    /// exists has nothing to attach to at all.
    pub fn ended(&mut self, pane: &PaneKey, now: u64, ending: Ending) -> Decision {
        let held = self.started.get(pane).copied().unwrap_or_default();
        match ending {
            Ending::TakenOver => {
                self.finished(pane, held, true);
                return Decision::Yield;
            }
            Ending::Gone => {
                self.finished(pane, held, true);
                return Decision::Leave;
            }
            Ending::Lost | Ending::Refused => {}
        }
        let settled = self
            .started
            .get(pane)
            .is_none_or(|started| now.saturating_sub(started.asked_at) >= SETTLED_NS);
        let tried = if settled { 0 } else { held.tried };
        if tried >= LIMIT {
            self.finished(pane, held, false);
            return Decision::GiveUp(tried);
        }
        let tried = tried + 1;
        self.ask(pane, held, tried, now);
        Decision::Start(tried)
    }

    /// Nothing has dialed this pane and no bridge has ended, so nothing will ask on its own.
    ///
    /// The other half of the recovery, and the one this module could not do until it was told
    /// the wait was over. [`Respawns::ended`] answers a bridge that ran and died; nothing
    /// answered a bridge that was decided on and never started, because there was no exit to
    /// notice - so a pane in that state had no bridge for the life of the app process, agent
    /// and all (kan a_2KIPfvt7L).
    ///
    /// Whether the last ask has *ended* is the whole of the rule, and it is what keeps
    /// [`LIMIT`] meaning what it says. A pane whose bridges are starting and dying on sight
    /// has been answered already - `ended` either started another or stopped - and asking here
    /// would restart a ladder that has just stopped, or take back a pane `Yield`
    /// deliberately left to another window. A pane whose ask produced nothing at all has
    /// spawned no process, so there is no storm to guard against and the answer is to ask
    /// again.
    ///
    /// The caller paces this. The wait it comes from is one deadline long, which is the
    /// interval between asks and is far from the fraction of a second the limit exists for.
    ///
    /// Answers what the view should publish, or `None` for a pane to leave alone. Not a
    /// [`Decision`], because the number that carries is which attempt in a run of failures and
    /// this asks for no attempt in one - the ask is free, so it spends nothing.
    pub fn stalled(&mut self, pane: &PaneKey, now: u64) -> Option<u32> {
        let held = self.started.get(pane).copied().unwrap_or_default();
        if held.ended {
            return None;
        }
        self.ask(pane, held, held.tried, now);
        Some(self.restarts(pane))
    }

    /// Somebody has asked for this pane to get a bridge, and a person asking is not a retry.
    ///
    /// So the run of failures starts over rather than continuing: whoever ran this has usually
    /// just done something about the cause - brought the machine back, or the network to it -
    /// and the next bridge deserves its own tries. It is also the only way back for a pane the
    /// ladder has stopped rebuilding, which is what makes stopping affordable.
    ///
    /// Returns what the view will publish, so the caller can say so.
    pub fn asked(&mut self, pane: &PaneKey, now: u64) -> u32 {
        let held = self.started.get(pane).copied().unwrap_or_default();
        self.ask(pane, held, 0, now);
        self.restarts(pane)
    }

    /// The daemon holding this pane answers again after it stopped, and nothing has dialed the
    /// pane.
    ///
    /// The run of failures starts over, as it does for a person asking: what ended the last
    /// bridges was most likely the connection that has just come back, and a pane the limit
    /// stopped rebuilding while it was down deserves its tries back. `None` for a pane left to
    /// another window or to its closing, which nothing automatic takes back.
    pub fn reconnected(&mut self, pane: &PaneKey, now: u64) -> Option<u32> {
        let held = self.started.get(pane).copied().unwrap_or_default();
        if held.left {
            return None;
        }
        self.ask(pane, held, 0, now);
        Some(self.restarts(pane))
    }

    /// Records a bridge asked for: one more for the pane, `tried` for the run of failures.
    fn ask(&mut self, pane: &PaneKey, held: Started, tried: u32, now: u64) {
        let started = Started {
            restarts: held.restarts + 1,
            tried,
            asked_at: now,
            left: false,
            ended: false,
        };
        self.started.insert(pane.clone(), started);
    }

    /// Records that the bridge asked for has ended without another being asked for.
    ///
    /// Kept rather than dropped, on the same terms as the count itself: a pane that gave up or
    /// yielded is one nothing is going to ask about again on its own, and the record is what
    /// says so.
    fn finished(&mut self, pane: &PaneKey, held: Started, left: bool) {
        self.started.insert(pane.clone(), Started { ended: true, left, ..held });
    }

    /// How many bridges this pane has been given, counting from zero for one nobody replaced.
    ///
    /// What the view carries. The shell builds a new surface whenever it changes, and a
    /// non-zero one is what tells its bridge it is re-attaching a pane this window held - which
    /// is when taking the pane over is the right thing rather than stealing it.
    pub fn restarts(&self, pane: &PaneKey) -> u32 {
        self.started.get(pane).map_or(0, |started| started.restarts)
    }

    /// The pane has gone, so what was tried for it means nothing.
    pub fn forget(&mut self, pane: &PaneKey) {
        self.started.remove(pane);
    }

    /// Keeps only the panes named, which is how a window that dropped a daemon lets go.
    pub fn retain(&mut self, keep: impl Fn(&PaneKey) -> bool) {
        self.started.retain(|pane, _| keep(pane));
    }
}

/// What to say about a pane another window is now drawing.
///
/// Not a failure, and worded so nobody reads it as one: everything is working, the pane is
/// being shown, and it is being shown somewhere else. What it has to carry is the way back,
/// because there is one and it is not obvious - a bridge asked for after the first takes the
/// pane, the same way the other window's did.
pub fn yielded(pane: &PaneKey) -> String {
    format!(
        "Another bridge attached to the pane {pane} and took it over, so this window has \
         stopped drawing it - most often a second Muster window that was opened onto the same \
         machine. The daemon lets one bridge draw a pane at a time, so nothing here can show it \
         while that one does; the agent itself is untouched and every other pane in this window is \
         unaffected. Whichever window is showing it now is the one to work in. To bring it \
         back here instead, run {} - that asks for a bridge, and a bridge after the first takes \
         the pane over the way the other window's did.",
        reattach_command(&pane.pane),
    )
}

/// What the run log should say about a pane Muster has stopped rebuilding.
///
/// Three things, because a warning that only says what happened leaves the reader starting
/// cold: what stopped, what it costs, and the cause worth checking first.
///
/// The roster is not told here, and does not need to be. The typeable watch restarts whenever a
/// bridge exits, so a pane nothing is dialing says so on its own row five seconds later - and
/// since it is told how the last bridge ended, it says this much there too.
pub fn gave_up(pane: &PaneKey, tried: u32) -> String {
    format!(
        "Muster started {tried} bridges for the pane {pane} and each one ended within \
         {} seconds, so it has stopped. This pane shows what it last painted, and what is typed \
         into it reaches the program without being drawn; every other pane in the window is \
         unaffected. The run log says why each \
         one ended - most often the bridge could not reach the daemon, which for a pane on \
         another machine is the ssh connection to it being down. Once that is fixed, {} asks \
         for another bridge, which is the way back that keeps the agent - closing the pane also \
         gets a fresh bridge, by ending what is running in it.",
        SETTLED_NS / 1_000_000_000,
        reattach_command(&pane.pane),
    )
}

/// How to ask this window for another bridge, as somebody would type it.
///
/// The name alone rather than a whole key, because the CLI finds a pane by name on every
/// machine a window shows - and because the bridge prints this too, and a bridge is told
/// which pane it is but not which daemon Muster calls the machine it runs on.
pub fn reattach_command(pane: &PaneId) -> String {
    format!("`muster pane reattach --pane {pane}`")
}
