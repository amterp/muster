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

pub(super) fn limit() -> Duration {
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
    /// When that process started, as it was when the wait was declared: a process at its pid
    /// that started at another time took the pid after it exited.
    pub(crate) process_started: Option<u64>,
    pub(crate) until_message_from: Option<String>,
    /// Whether it runs out after [`LIMIT`].
    pub(crate) limited: bool,
    pub(crate) started_ms: Option<u64>,
    pub(crate) noticed: bool,
    pub(crate) carried: bool,
}

/// What ended a wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Over {
    /// Its process exited.
    Exited,
    /// It ran out.
    RanOut,
    Message,
    /// The pane it waited on a message from closed.
    SenderClosed,
}

impl Over {
    /// As the run log says it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Over::Exited => "exit",
            Over::RanOut => "limit",
            Over::Message => "message",
            Over::SenderClosed => "sender_closed",
        }
    }
}

/// Whether process `pid`, as `report --until-exits` names it, is still there and running: not a
/// zombie awaiting its reaping, and, given when it `started`, not another process that has
/// taken its pid since.
pub(crate) fn exists(pid: u32, started: Option<u64>) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(pty::process_started)
        .is_some_and(|now| started.is_none_or(|then| then == now))
}

/// Messages about to reach an agent, and the wait on a message from one of their authors that
/// delivering them ends. Captured when they are queued and finished once the agent has them.
#[derive(Debug)]
pub(super) struct MessageWait {
    pane: String,
    /// The panes the messages are from.
    authors: Vec<String>,
    /// The serial of the agent's wait on one of those panes, if it holds one.
    ends: Option<u64>,
}

impl MessageWait {
    pub(super) fn for_notice(
        shared: &Shared,
        reader: &str,
        group: &str,
        first: u64,
        last: u64,
    ) -> Self {
        let authors = shared.messages().service.notice_authors(reader, group, first, last);
        Self::capture(shared, reader, &authors)
    }

    pub(super) fn capture(shared: &Shared, reader: &str, authors: &[String]) -> Self {
        let (pane, authors) = {
            let messages = shared.messages();
            let pane = pane_of(&messages, reader);
            let authors: Vec<String> =
                authors.iter().map(|author| pane_of(&messages, author)).collect();
            (pane, authors)
        };
        let ends = shared
            .lock()
            .declared_waits()
            .into_iter()
            .find(|wait| {
                wait.pane == pane
                    && wait.until_message_from.as_ref().is_some_and(|from| authors.contains(from))
            })
            .map(|wait| wait.serial);
        Self { pane, authors, ends }
    }

    pub(super) fn finish(self, shared: &Shared) {
        let mut session = shared.lock();
        if let Some(serial) = self.ends {
            session.wait_over(&self.pane, serial, Over::Message);
        }
        session.heard_from(&self.pane, self.authors);
    }
}

/// The pane `name` is the agent in, or `name` itself when it is not known as a participant.
fn pane_of(messages: &super::Messages, name: &str) -> String {
    messages
        .service
        .participant(name)
        .and_then(|p| p.pane.clone())
        .unwrap_or_else(|| name.to_string())
}

/// `author` posted to `reached`: what they answered before is not the answer to what it asked
/// now, so a wait on them declared next is for something new.
pub(super) fn posted(shared: &Shared, author: &str, reached: &[(String, muster_msg::Reach)]) {
    let (pane, recipients) = {
        let messages = shared.messages();
        let recipients: Vec<String> =
            reached.iter().map(|(name, _)| pane_of(&messages, name)).collect();
        (pane_of(&messages, author), recipients)
    };
    shared.lock().forget_heard_from(&pane, &recipients);
}

pub(super) fn heard(shared: &Shared, name: &str, authors: &[String]) {
    MessageWait::capture(shared, name, authors).finish(shared);
}

pub(super) fn heard_notice(shared: &Shared, name: &str, group: &str, first: u64, last: u64) {
    MessageWait::for_notice(shared, name, group, first, last).finish(shared);
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
        exited: impl Fn(u32, Option<u64>) -> bool,
    ) -> (Vec<Ended>, Duration) {
        self.since.retain(|pane, _| held.iter().any(|wait| wait.pane == *pane && wait.limited));
        let mut ended = Vec::new();
        let mut next = NOTHING_DUE;
        for wait in held {
            let over = |over| Ended { pane: wait.pane.clone(), serial: wait.serial, over };
            if wait.until_message_from.is_some() {
                continue;
            }
            if let Some(pid) = wait.until_exits {
                if exited(pid, wait.process_started) {
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
    let (ended, mut next) = waits.judge(&held, now, |pid, started| !exists(pid, started));
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
        // The pane reads idle now, which the maker watch weighs on the thread's next pass: this
        // watch runs last, so that pass would otherwise wait for something else to wake it.
        next = Duration::ZERO;
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
            process_started: None,
            until_message_from: None,
            limited,
            started_ms: None,
            noticed: false,
            carried: false,
        }
    }

    fn overs(ended: &[Ended]) -> Vec<(&str, Over)> {
        ended.iter().map(|ended| (ended.pane.as_str(), ended.over)).collect()
    }

    /// A child that has exited and that nobody has reaped yet is a zombie: still in the process
    /// table, so asking whether its pid exists says yes, though it will never run again.
    #[test]
    fn a_process_that_exited_unreaped_is_not_there() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().expect("true runs");
        std::thread::sleep(Duration::from_millis(300));
        let there = exists(child.id(), None);
        child.wait().expect("the child is reaped");
        assert!(!there, "a zombie read as a running process");
    }

    /// A pid held by a process that started at another time than the one waited on names a
    /// different process, which took the pid after the first exited.
    #[test]
    fn a_pid_taken_by_another_process_is_not_the_one_waited_on() {
        let pid = std::process::id();
        let started = pty::process_started(i32::try_from(pid).expect("a pid fits"));
        assert!(started.is_some(), "this test's own process reads as started");
        assert!(exists(pid, started), "the same process is still there");
        assert!(!exists(pid, started.map(|started| started + 1)), "another process took the pid");
    }

    /// A wait on a process ends once the process has exited, and is looked at every second
    /// until then; one nothing limits is left alone.
    #[test]
    fn a_wait_on_a_process_ends_when_it_exits() {
        let mut waits = Waits::after(Duration::from_millis(LIMIT_MS));
        let start = Instant::now();
        let held = [wait("p1", 1, Some(4242), false), wait("p2", 1, None, false)];
        let (ended, next) = waits.judge(&held, start, |_, _| false);
        assert!(ended.is_empty());
        assert_eq!(next, EXIT_LOOK);
        let (ended, _) = waits.judge(&held, start + EXIT_LOOK, |pid, _| pid == 4242);
        assert_eq!(overs(&ended), [("p1", Over::Exited)]);

        let (ended, next) = waits.judge(&held[1..], start + LIMIT * 2, |_, _| true);
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
        let (_, next) = waits.judge(&[wait("p1", 1, None, true)], start, |_, _| false);
        assert_eq!(next, Duration::from_secs(30));

        let (ended, next) = waits.judge(&[wait("p1", 2, None, true)], seconds(20), |_, _| false);
        assert!(ended.is_empty(), "declared again");
        assert_eq!(next, Duration::from_secs(30));
        let (ended, _) = waits.judge(&[wait("p1", 2, None, true)], seconds(45), |_, _| false);
        assert!(ended.is_empty());
        let (ended, _) = waits.judge(&[wait("p1", 2, None, true)], seconds(50), |_, _| false);
        assert_eq!(overs(&ended), [("p1", Over::RanOut)]);
    }

    /// A wait on a process is not limited, whatever else is said of it.
    #[test]
    fn a_wait_on_a_process_does_not_run_out() {
        let mut waits = Waits::after(Duration::from_millis(LIMIT_MS));
        let start = Instant::now();
        let held = [wait("p1", 1, Some(4242), true)];
        waits.judge(&held, start, |_, _| false);
        let (ended, _) = waits.judge(&held, start + Duration::from_hours(1), |_, _| false);
        assert!(ended.is_empty());
    }
}
