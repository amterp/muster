//! A wait an agent declared that its next turn does not end. One declared until a process exits
//! (`report --until-exits`) ends when the process does, and the agent is told so in a message
//! from `@muster`, which wakes it as any message does: Codex has nothing that starts a turn when
//! a job of its own ends. One carried through a turn Muster typed runs out after [`LIMIT`], since
//! the agent may have been handed what it waited on inside that turn, and then no later turn
//! comes to end the wait.
//!
//! Watched on the doorbell's thread, which already reads a clock for the maker watch: the session
//! reads none of its own.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;

use super::presence::Panes;
use super::{delivering, now_ms, told};
use crate::pty;
use crate::session::Shared;

/// How long a wait that nothing is known to end lasts: as long as one the screen shows.
const LIMIT: Duration = muster_detect::Detector::SCREEN_WAIT;

/// A debug build's [`LIMIT`] in milliseconds, when set, so a test need not wait for it.
const LIMIT_SAID: &str = "MUSTER_DAEMON_DECLARED_WAIT_MS";

/// How often a process waited on is looked for while one is.
const EXIT_LOOK: Duration = Duration::from_secs(1);

/// How long the doorbell's thread may sleep for all this watch cares, with no wait running out
/// and no process waited on: a declaration nudges it anyway.
const NOTHING_DUE: Duration = Duration::from_mins(1);

fn limit() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(LIMIT_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    LIMIT
}

/// A wait an agent declared and holds, as the session has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) pane: String,
    /// What the agent said it waits on.
    pub(crate) waiting: String,
    /// Which of the pane's waits it is ([`crate::pane::Turns`]).
    pub(crate) serial: u64,
    /// The process whose exit ends it.
    pub(crate) until_exits: Option<u32>,
    /// Whether it runs out after [`LIMIT`].
    pub(crate) limited: bool,
}

/// What ended a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Over {
    /// Its process exited.
    Exited,
    /// It ran out.
    RanOut,
}

impl Over {
    /// As the run log says it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Over::Exited => "exit",
            Over::RanOut => "limit",
        }
    }
}

/// Whether process `pid`, as `report --until-exits` names it, is still there.
pub(crate) fn exists(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(pty::process_exists)
}

/// A wait ended, by what.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ended {
    pane: String,
    serial: u64,
    over: Over,
}

/// When this watch first saw each limited wait, by pane, with its serial.
#[derive(Debug)]
pub(crate) struct Waits {
    limit: Duration,
    since: HashMap<String, (u64, Instant)>,
    /// Waits on a process that exited whose agent could not be told yet, by pane and serial,
    /// with when that was first so: said once each, tried again at each look, and given up
    /// after the limit, as a wait nothing ends is.
    untold: HashMap<(String, u64), Instant>,
}

impl Waits {
    pub(crate) fn new() -> Waits {
        Waits::after(limit())
    }

    fn after(limit: Duration) -> Waits {
        Waits { limit, since: HashMap::new(), untold: HashMap::new() }
    }

    /// Which of the waits `held` are over at `now`, `exited` saying whether a process has, and
    /// how long until the next look is due. Pure, so every rule here is a unit test.
    fn judge(
        &mut self,
        held: &[Declared],
        now: Instant,
        exited: impl Fn(u32) -> bool,
    ) -> (Vec<Ended>, Duration) {
        self.since.retain(|pane, _| held.iter().any(|wait| wait.pane == *pane && wait.limited));
        let mut ended = Vec::new();
        let mut next = NOTHING_DUE;
        for wait in held {
            let over = |over| Ended { pane: wait.pane.clone(), serial: wait.serial, over };
            if let Some(pid) = wait.until_exits {
                if exited(pid) {
                    ended.push(over(Over::Exited));
                } else {
                    next = next.min(EXIT_LOOK);
                }
                continue;
            }
            if !wait.limited {
                continue;
            }
            let since = match self.since.get(&wait.pane) {
                Some(&(serial, since)) if serial == wait.serial => since,
                _ => {
                    self.since.insert(wait.pane.clone(), (wait.serial, now));
                    now
                }
            };
            let left = self.limit.saturating_sub(now.saturating_duration_since(since));
            if left.is_zero() {
                self.since.remove(&wait.pane);
                ended.push(over(Over::RanOut));
            } else {
                next = next.min(left);
            }
        }
        (ended, next)
    }
}

/// Ends each declared wait that is over, and tells an agent whose process exited. Called by the
/// doorbell's thread with no lock held, on every wake; says how long until the next look is due.
pub(crate) fn look(shared: &Arc<Shared>, waits: &mut Waits) -> Duration {
    // A daemon handing over refuses every post, so a wait ended now would be cleared with nobody
    // told. The newer daemon takes the wait over with the pane's facts and ends it there.
    if shared.messages().handing_over {
        return NOTHING_DUE;
    }
    let held = shared.lock().declared_waits();
    if held.is_empty() {
        waits.since.clear();
        waits.untold.clear();
        return NOTHING_DUE;
    }
    waits.untold.retain(|(pane, serial), _| {
        held.iter().any(|wait| wait.pane == *pane && wait.serial == *serial)
    });
    let now = Instant::now();
    let (ended, mut next) = waits.judge(&held, now, |pid| !exists(pid));
    for Ended { pane, serial, over } in ended {
        let Some(wait) = held.iter().find(|wait| wait.pane == pane) else { continue };
        // The agent is told before its wait ends: a wait cleared and its wake refused would
        // leave it idle with nothing to start its next turn.
        if let Some(pid) = wait.until_exits.filter(|_| over == Over::Exited) {
            // Still the wait this look saw: one the agent cleared or replaced meanwhile is not
            // over, and its agent is not told it is.
            if !shared.lock().holds_wait(&pane, serial) {
                continue;
            }
            match tell(shared, &pane, &wait.waiting, pid) {
                Ok(()) => {
                    waits.untold.remove(&(pane.clone(), serial));
                }
                Err(reason) => {
                    let key = (pane.clone(), serial);
                    let first = *waits.untold.entry(key).or_insert(now);
                    if first == now {
                        untold(&pane, pid, &reason);
                    }
                    if now.saturating_duration_since(first) < waits.limit {
                        next = next.min(EXIT_LOOK);
                        continue;
                    }
                    given_up(&pane, pid, &reason);
                }
            }
        }
        if !shared.lock().wait_over(&pane, serial, over) {
            continue;
        }
        log::info(
            "daemon.wait.over",
            fields! {
                "pane" => pane,
                "by" => over.as_str(),
                "waited_pid" => wait.until_exits.map(|pid| pid.to_string()).unwrap_or_default(),
            },
        );
    }
    next
}

/// Tells the agent in `pane` that its wait on `waited` is over, in a message of its own, or says
/// why it could not.
fn tell(shared: &Arc<Shared>, pane: &str, waited: &str, pid: u32) -> Result<(), String> {
    let panes = Panes::of(shared);
    let body = format!("Your wait on {waited} is over: process {pid} has exited.");
    let delivered = delivering(shared, &panes, |service| {
        service.tell_alone_as_muster(pane, &body, &panes, now_ms())
    });
    let delivered = delivered.map_err(|reply| reply.reason)?;
    told("msg.wait.told", Some(body.len()), &delivered);
    Ok(())
}

fn given_up(pane: &str, pid: u32, reason: &str) {
    log::warn(
        "msg.wait.given_up",
        fields! {
            "pane" => pane,
            "waited_pid" => pid,
            "reason" => reason,
            "impact" => "the agent's wait ends untold: its pane reads idle, and nothing starts \
                         its next turn; somebody has to prompt it",
            "check" => "msg.wait.untold's reason, logged when this began; `muster pane send` \
                        can prompt the agent",
        },
    );
}

fn untold(pane: &str, pid: u32, reason: &str) {
    log::warn(
        "msg.wait.untold",
        fields! {
            "pane" => pane,
            "waited_pid" => pid,
            "reason" => reason,
            "impact" => "the agent's wait stands and it is not woken until it can be told: its \
                         pane reads waiting though the work it waited on has ended; tried again \
                         every second, and given up after the limit a wait nothing ends has",
            "check" => "the reason: a store that would not write, or a handoff under way; \
                        `muster pane send` can prompt the agent",
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT_MS: u64 = 600_000;

    fn wait(pane: &str, serial: u64, until_exits: Option<u32>, limited: bool) -> Declared {
        Declared {
            pane: pane.to_string(),
            waiting: "the gate".to_string(),
            serial,
            until_exits,
            limited,
        }
    }

    fn overs(ended: &[Ended]) -> Vec<(&str, Over)> {
        ended.iter().map(|ended| (ended.pane.as_str(), ended.over)).collect()
    }

    /// A wait on a process ends once the process has exited, and is looked at every second
    /// until then; one nothing limits is left alone.
    #[test]
    fn a_wait_on_a_process_ends_when_it_exits() {
        let mut waits = Waits::after(Duration::from_millis(LIMIT_MS));
        let start = Instant::now();
        let held = [wait("p1", 1, Some(4242), false), wait("p2", 1, None, false)];
        let (ended, next) = waits.judge(&held, start, |_| false);
        assert!(ended.is_empty());
        assert_eq!(next, EXIT_LOOK);
        let (ended, _) = waits.judge(&held, start + EXIT_LOOK, |pid| pid == 4242);
        assert_eq!(overs(&ended), [("p1", Over::Exited)]);

        let (ended, next) = waits.judge(&held[1..], start + LIMIT * 2, |_| true);
        assert!(ended.is_empty(), "a wait nothing limits does not run out");
        assert_eq!(next, NOTHING_DUE);
    }

    /// A limited wait runs out the limit after this watch first saw it, and one declared again
    /// starts the limit over.
    #[test]
    fn a_limited_wait_runs_out_and_starts_over_when_declared_again() {
        let mut waits = Waits::after(Duration::from_secs(30));
        let start = Instant::now();
        let seconds = |seconds| start + Duration::from_secs(seconds);
        let (_, next) = waits.judge(&[wait("p1", 1, None, true)], start, |_| false);
        assert_eq!(next, Duration::from_secs(30));

        let (ended, next) = waits.judge(&[wait("p1", 2, None, true)], seconds(20), |_| false);
        assert!(ended.is_empty(), "declared again");
        assert_eq!(next, Duration::from_secs(30));
        let (ended, _) = waits.judge(&[wait("p1", 2, None, true)], seconds(45), |_| false);
        assert!(ended.is_empty());
        let (ended, _) = waits.judge(&[wait("p1", 2, None, true)], seconds(50), |_| false);
        assert_eq!(overs(&ended), [("p1", Over::RanOut)]);
    }

    /// A wait on a process is not limited, whatever else is said of it.
    #[test]
    fn a_wait_on_a_process_does_not_run_out() {
        let mut waits = Waits::after(Duration::from_millis(LIMIT_MS));
        let start = Instant::now();
        let held = [wait("p1", 1, Some(4242), true)];
        waits.judge(&held, start, |_| false);
        let (ended, _) = waits.judge(&held, start + Duration::from_hours(1), |_| false);
        assert!(ended.is_empty());
    }
}
