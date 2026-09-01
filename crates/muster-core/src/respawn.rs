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

/// What to do about a bridge that has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Start another one. Carries which replacement this is, counting from one, which the view
    /// publishes so the shell knows to build a new surface and that this is a re-attach.
    Start(u32),

    /// Stop. Carries how many were tried, for the sentence the run log carries.
    GiveUp(u32),
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
    pub fn ended(&mut self, pane: &PaneKey, now: u64) -> Decision {
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
