//! How hard to try a connection that keeps failing, and when to say so.
//!
//! The sibling of `respawn`, and written from the same measurement: something that keeps
//! failing must be retried at a rate that reflects how likely it is to work, and the thing
//! that decides that is how long the last success lasted. Plugging a laptop back in produced
//! 97 down/reopened pairs in 118 seconds, a new one every 1.26s, never settling and never
//! escalating - because the loop reset its backoff whenever the child it had just spawned was
//! still alive at the next poll, and an ssh that lives a quarter of a second before losing its
//! forward is alive at the next poll (kan a_2IRdZK6Un).
//!
//! Two differences from the bridge policy, both deliberate.
//!
//! **A confirmed success, not a started one.** Spawning a process is not connecting, and
//! `attempt = 0` on a process that exists is what made the flap perpetual. Only the caller can
//! say what confirmation means; this module only requires that it be told.
//!
//! **It reports and keeps trying rather than giving up.** A bridge that gives up costs one
//! pane, and its remedy is to close that pane and open it again. A tunnel that gave up costs
//! every pane on that machine and its remedy is to relaunch the app, which is the failure the
//! whole recovery story exists to prevent - a laptop that comes back from lunch should find
//! its window working. So the ceiling is a long interval rather than a stop, and what happens
//! at the point a bridge would give up is that somebody is told.
//!
//! Pure - no clock, no processes, no sockets. Time arrives as a number, so every rule here is
//! driven by a recorded case.

/// How long to wait before each attempt, in nanoseconds.
///
/// Quick at first because the overwhelmingly common drop is a route changing under a laptop
/// and coming straight back, and a window that took ten seconds to notice would be a window
/// people relaunch out of habit. Slow at the end because the other common case is a machine
/// that is not coming back this hour, and a connection retried every second all afternoon is
/// noise in the log and a process spawned three thousand times.
///
/// The last entry is the ceiling and stays in force forever.
pub const BACKOFF_NS: [u64; 7] = [
    50_000_000,     // 50ms
    200_000_000,    // 200ms
    500_000_000,    // 500ms
    1_000_000_000,  // 1s
    5_000_000_000,  // 5s
    15_000_000_000, // 15s
    30_000_000_000, // 30s
];

/// How many attempts may fail before the person is told rather than only the log.
///
/// Five, which under the table above is about seven seconds. Long enough that a route
/// changing under a laptop is fixed before anybody is interrupted, short enough that somebody
/// who has walked back into range of nothing finds out from the window rather than by typing.
pub const PATIENCE: u32 = 5;

/// How long a connection must hold before it counts as having worked, in nanoseconds.
///
/// Thirty seconds, matching `respawn::SETTLED_NS` and for the same reason: it is well past
/// what a connection takes to establish and well short of any interval a person would call
/// "it was fine and then it broke". A connection that only ever lasts a second is one attempt
/// in a run of failures however many times it comes back, which is what a flap is.
pub const SETTLED_NS: u64 = 30_000_000_000;

/// What to do about a connection that has just failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Which attempt the next one will be, counting from one.
    pub attempt: u32,

    /// How long to wait first, in nanoseconds.
    pub after: u64,

    /// Whether this is the attempt at which the person should be told.
    ///
    /// True once per run of failures, at `PATIENCE`, rather than on every attempt after it: a
    /// condition that stays true has nothing new to say, which is the rule the problem list is
    /// built on.
    pub report: bool,
}

/// One connection's run of failures.
#[derive(Debug, Default, Clone, Copy)]
pub struct Attempts {
    failures: u32,
    /// Whether the person has been told about this run.
    reported: bool,
    /// When the current stretch of being up began, if it is up.
    up_since: Option<u64>,
}

impl Attempts {
    pub const fn new() -> Attempts {
        Attempts { failures: 0, reported: false, up_since: None }
    }

    /// An attempt failed, or a connection that was up went down.
    ///
    /// Takes no clock: the run of failures is ended by holding, not by the passage of time, so
    /// nothing here has a moment to compare against.
    pub fn failed(&mut self) -> Retry {
        self.up_since = None;
        self.failures += 1;
        let report = self.failures >= PATIENCE && !self.reported;
        self.reported |= report;
        let step = usize::try_from(self.failures - 1).unwrap_or(usize::MAX);
        Retry { attempt: self.failures, after: BACKOFF_NS[step.min(BACKOFF_NS.len() - 1)], report }
    }

    /// The connection is up right now, and something checked rather than assumed it.
    ///
    /// Answers whether a run of failures has just ended, which is the moment a problem is
    /// taken back. It ends when the connection has *held* for the settling time, not when it
    /// came up: a connection that comes up and falls over inside that window is the same run
    /// continuing, whatever it looked like in between, and treating it as a success is exactly
    /// what let a laptop plugged back in flap once a second for two minutes.
    ///
    /// Called repeatedly while the connection is up, so it has to be cheap for the caller and
    /// silent here once there is nothing left to report.
    pub fn holding(&mut self, now: u64) -> bool {
        let Some(since) = self.up_since else {
            self.up_since = Some(now);
            return false;
        };
        if now.saturating_sub(since) < SETTLED_NS {
            return false;
        }
        let recovered = self.reported;
        self.failures = 0;
        self.reported = false;
        recovered
    }

    /// How many attempts have failed in the current run.
    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// What to tell somebody whose machine has stopped answering.
///
/// The three things a warning owes: what stopped, what it costs, and what to check. It says
/// Muster is still trying, because that is the difference between a window somebody relaunches
/// and one they leave alone - and relaunching is what costs every pane on the machines that
/// were fine.
pub fn unreachable(host: &str, failures: u32) -> String {
    format!(
        "Muster has tried {failures} times to reopen the connection to {host} and none of them \
         has held. Every pane on that machine is showing what it last painted and its agent \
         states are a guess about the present; panes on other machines in this window are \
         unaffected, and so is the work on {host} itself - the daemon there keeps running and \
         its agents keep going. Check that {host} is reachable, and whether a VPN needs \
         reconnecting. Muster keeps trying about every {} seconds and the panes come back on \
         their own once it answers, so relaunching is not necessary and would cost the panes \
         on every other machine.",
        BACKOFF_NS[BACKOFF_NS.len() - 1] / 1_000_000_000,
    )
}

/// The key the problem list files that under, one per machine.
///
/// A machine rather than a window, because a laptop and a devenv fail independently and a
/// window-wide answer would accuse the one that is working.
pub fn key(daemon: &str) -> String {
    format!("daemon:{daemon}")
}
