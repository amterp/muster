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
//! network change the daemon holds it and the far machine refuses the attach anyway, because
//! the herdr client from before the change is still there with the terminal. So the thing that
//! separates them is how long the last bridge lasted. One that ran for an hour and then died
//! is a connection; one that died on sight, three times inside half a minute, is not going to
//! work on the fourth try either.
//!
//! Dialing back cannot be the health signal, which is worth stating because it is the obvious
//! candidate. A bridge whose attach is refused still reaches the app first - it dials, then
//! runs herdr, then reports the refusal and exits - so a rule that reset on a dial would reset
//! on exactly the failure it is meant to stop.
//!
//! Pure - no clock, no processes. Time arrives as a number, so every rule here is driven by a
//! recorded case.

use std::collections::BTreeMap;

use crate::composition::PaneKey;

/// How many replacements a pane gets before Muster stops and says so.
///
/// Three, because the case worth surviving is a network that comes back within a few seconds
/// and the case worth stopping is one that never will. Two would give up on a machine that
/// takes one extra moment to let go of a terminal; ten would be forty seconds of spawning
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
/// Three, because they are the three the app has to answer differently. The daemon says only
/// what happened, in prose; `muster_herdr::bridge_report` is where that becomes one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The stream carrying it ended. A route that changed, a daemon that restarted, a pane
    /// that closed - and from here they are one thing, because the answer to all three is to
    /// look again and start another bridge if the pane is still there.
    Lost,

    /// The attach was refused: something else already holds this pane's terminal.
    ///
    /// Ordinary after a relaunch. A herdr client whose transport died goes on holding the
    /// terminal, so the first attach of a fresh app is refused by a machine that is otherwise
    /// perfectly healthy (kan a_2I76eCrjw).
    Refused,

    /// Another client attached and herdr handed the terminal over.
    ///
    /// The one ending that must not be answered by attaching again. Somebody asked for this
    /// pane somewhere else and got it; taking it back would be answered the same way, and two
    /// windows would trade one terminal until both gave up.
    TakenOver,
}

impl Ending {
    /// The word for the wire and the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::Lost => "lost",
            Ending::Refused => "refused",
            Ending::TakenOver => "taken_over",
        }
    }

    pub fn parse(word: &str) -> Option<Ending> {
        match word {
            "lost" => Some(Ending::Lost),
            "refused" => Some(Ending::Refused),
            "taken_over" => Some(Ending::TakenOver),
            _ => None,
        }
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

    /// Leave this pane's terminal to whoever now has it.
    ///
    /// Carries nothing and changes nothing, deliberately. The count is what the view
    /// publishes and a count that moved is what makes the shell build a new surface, so a
    /// yield that touched it would start the bridge it is refusing to start.
    Yield,
}

/// Every pane whose bridge Muster has replaced, and how recently.
#[derive(Debug, Default)]
pub struct Respawns {
    started: BTreeMap<PaneKey, Started>,
}

#[derive(Debug, Clone, Copy)]
struct Started {
    /// How many replacements have been asked for in the current run of failures.
    count: u32,
    /// When the last one was asked for, on whatever monotonic scale the caller counts in.
    ///
    /// Stands in for how long that bridge lived, which is what the rule is really about: the
    /// shell starts one within a frame of being told to, so the gap between asking and the
    /// next exit is the bridge's life to within a few milliseconds.
    asked_at: u64,
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
    /// all. For two of the three it is - a connection that went and a terminal held by a client
    /// that has not noticed its transport died are both recovered by attaching again, and the
    /// second needs the `--takeover` a replacement carries. For the third it is not. A terminal
    /// handed to another client was handed to somebody who asked for it, and taking it back
    /// would be answered the same way from the other side: two windows trading one terminal
    /// at the speed a bridge starts, until both of them ran out of tries.
    pub fn ended(&mut self, pane: &PaneKey, now: u64, ending: Ending) -> Decision {
        if ending == Ending::TakenOver {
            return Decision::Yield;
        }
        let settled = self
            .started
            .get(pane)
            .is_none_or(|started| now.saturating_sub(started.asked_at) >= SETTLED_NS);
        let tried = if settled { 0 } else { self.started[pane].count };
        if tried >= LIMIT {
            return Decision::GiveUp(tried);
        }
        let count = tried + 1;
        self.started.insert(pane.clone(), Started { count, asked_at: now });
        Decision::Start(count)
    }

    /// Which replacement this pane's bridge is on, counting from zero for one nobody replaced.
    ///
    /// What the view carries. The shell builds a new surface whenever it changes, and a
    /// non-zero one is what tells its bridge it is re-attaching a pane this window held - which
    /// is when taking the terminal over is the right thing rather than stealing it.
    pub fn count(&self, pane: &PaneKey) -> u32 {
        self.started.get(pane).map_or(0, |started| started.count)
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

/// What to say about a pane whose terminal is now somebody else's.
///
/// Not a failure, and worded so nobody reads it as one: everything is working, the pane is
/// being shown, and it is being shown somewhere else. What it has to carry is the way back,
/// because there is one and it is not obvious - closing the pane here and opening it again
/// starts a bridge that does take the terminal, which is the same action that recovers every
/// other stuck pane.
pub fn yielded(pane: &PaneKey) -> String {
    format!(
        "Another client attached to the pane {pane} and took its terminal, so this window has \
         stopped drawing it - most often a second Muster window that was opened onto the same \
         machine. Only one client may hold a herdr terminal, so nothing here can show it while \
         that one does; the agent itself is untouched and every other pane in this window is \
         unaffected. Whichever window is showing it now is the one to type into. To bring it \
         back here instead, close this pane and open it again - the fresh bridge takes the \
         terminal the way that one did.",
    )
}

/// What the run log should say about a pane Muster has stopped rebuilding.
///
/// Three things, because a warning that only says what happened leaves the reader starting
/// cold: what stopped, what it costs, and the causes worth checking first. The orphaned client
/// is named because it is the one this was written for and the one nobody guesses - a herdr
/// client whose ssh died goes on holding its terminal, so every later attach is refused by a
/// machine that looks perfectly healthy.
///
/// The roster is not told, and does not need to be. The typeable watch restarts whenever a
/// bridge exits, so a pane nothing is dialing says so on its own row five seconds later.
pub fn gave_up(pane: &PaneKey, tried: u32) -> String {
    format!(
        "Muster started {tried} bridges for the pane {pane} and each one ended within \
         {} seconds, so it has stopped. This pane shows what it last painted and takes no \
         keystrokes; every other pane in the window is unaffected. The run log says why each \
         one ended - a `bridge.attach.failed` there means the pane's terminal is still held by \
         a client from before, most often one on the far machine whose ssh died with the \
         network, and `ssh <host> 'pkill -f \"terminal session control {}\"'` releases it. \
         Closing this pane and opening it again starts a fresh bridge.",
        SETTLED_NS / 1_000_000_000,
        pane.pane,
    )
}
