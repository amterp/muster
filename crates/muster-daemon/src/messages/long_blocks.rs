//! A pane left at a dialog for hours is told to the person, once for each block. The stall watch
//! tells them of a dialog only when a message waits on it, and the maker watch only of a helper's,
//! so an agent that asked a question nobody is waiting on would otherwise sit until someone
//! happens to look: the one-time blocked banner is easy to miss, and two hours is longer than
//! anyone at the window leaves a dialog they can see.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// How long a pane must sit at a dialog before the person is told.
const LONG_BLOCK: Duration = Duration::from_hours(2);

/// A debug build's [`LONG_BLOCK`] in milliseconds, when set, so a test need not wait for it.
const LONG_BLOCK_SAID: &str = "MUSTER_DAEMON_LONG_BLOCK_MS";

fn long_block() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(LONG_BLOCK_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    LONG_BLOCK
}

/// A pane at a dialog, as a look found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Blocked<'a> {
    pub(crate) pane: &'a str,
    /// Whether it is a helper, whose maker is told and then the person (`super::makers`).
    pub(crate) helper: bool,
    /// Whether a message waits on it, which the stall watch tells of.
    pub(crate) unread: bool,
}

#[derive(Debug, Clone, Copy)]
struct Block {
    since_ms: u64,
    told: bool,
}

/// Every pane at a dialog, between looks.
#[derive(Debug)]
pub(crate) struct LongBlocks {
    limit_ms: u64,
    blocks: HashMap<String, Block>,
}

impl LongBlocks {
    pub(crate) fn new() -> LongBlocks {
        LongBlocks::after(long_block())
    }

    fn after(limit: Duration) -> LongBlocks {
        LongBlocks {
            limit_ms: u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
            blocks: HashMap::new(),
        }
    }

    /// Weighs each pane at a dialog now and says which to tell the person of, with how long each
    /// has been there. `escalated` holds the panes the stall watch told the person of at this
    /// look, which this block is then told of already. Pure, so every rule here is a unit test.
    pub(crate) fn judge(
        &mut self,
        blocked: &[Blocked<'_>],
        escalated: &HashSet<&str>,
        in_reach: bool,
        now_ms: u64,
    ) -> Vec<(String, u64)> {
        // A pane no longer at a dialog has ended its block, and a later one is told afresh.
        self.blocks.retain(|pane, _| blocked.iter().any(|at| at.pane == pane && !at.helper));
        let mut due = Vec::new();
        for at in blocked.iter().filter(|at| !at.helper) {
            let block = self
                .blocks
                .entry(at.pane.to_string())
                .or_insert(Block { since_ms: now_ms, told: false });
            block.told |= escalated.contains(at.pane);
            let held_ms = now_ms.saturating_sub(block.since_ms);
            if block.told || at.unread || held_ms < self.limit_ms || !in_reach {
                continue;
            }
            block.told = true;
            due.push((at.pane.to_string(), held_ms));
        }
        due
    }
}

/// What the person is told of `pane`, called `called`, at a dialog for `held_ms`.
pub(crate) fn body(pane: &str, called: Option<&str>, held_ms: u64) -> String {
    let named = called.map_or_else(|| pane.to_string(), |called| format!("{called} ({pane})"));
    let by = called.unwrap_or(pane);
    format!(
        "{named} has been at a dialog for {held}, {why}, and nothing is waiting on it. Only \
         somebody at its pane can answer it, so Muster tells you, once; `muster pane show --pane \
         {by}` brings it forward.",
        held = super::stalls::said_as_time(held_ms),
        why = super::stalls::AT_A_DIALOG,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: u64 = 3_600_000;

    fn at(pane: &str) -> Blocked<'_> {
        Blocked { pane, helper: false, unread: false }
    }

    fn told(watch: &mut LongBlocks, blocked: &[Blocked<'_>], now_ms: u64) -> Vec<String> {
        watch
            .judge(blocked, &HashSet::new(), true, now_ms)
            .into_iter()
            .map(|(pane, _)| pane)
            .collect()
    }

    #[test]
    fn a_dialog_left_two_hours_is_told_once_and_a_later_block_afresh() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        assert!(told(&mut watch, &[at("p1")], 0).is_empty());
        assert!(told(&mut watch, &[at("p1")], 2 * HOUR_MS - 1).is_empty());
        assert_eq!(told(&mut watch, &[at("p1")], 2 * HOUR_MS), ["p1"]);
        assert!(told(&mut watch, &[at("p1")], 3 * HOUR_MS).is_empty(), "told twice");

        assert!(told(&mut watch, &[], 4 * HOUR_MS).is_empty());
        assert!(told(&mut watch, &[at("p1")], 5 * HOUR_MS).is_empty(), "a new block starts over");
        assert_eq!(told(&mut watch, &[at("p1")], 7 * HOUR_MS), ["p1"]);
    }

    #[test]
    fn a_helper_or_a_dialog_a_message_waits_on_is_left_to_the_other_watches() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        let helper = Blocked { helper: true, ..at("p2") };
        let unread = Blocked { unread: true, ..at("p3") };
        told(&mut watch, &[helper, unread], 0);
        assert!(told(&mut watch, &[helper, unread], 3 * HOUR_MS).is_empty());
        assert_eq!(told(&mut watch, &[helper, at("p3")], 3 * HOUR_MS + 1), ["p3"], "read since");
    }

    #[test]
    fn a_block_the_stall_watch_told_the_person_of_is_not_told_again() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        let unread = Blocked { unread: true, ..at("p1") };
        watch.judge(&[unread], &HashSet::new(), true, 0);
        watch.judge(&[unread], &HashSet::from(["p1"]), true, HOUR_MS / 2);
        assert!(told(&mut watch, &[at("p1")], 3 * HOUR_MS).is_empty());
    }

    #[test]
    fn a_block_waits_for_the_person_to_be_in_reach() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        assert!(watch.judge(&[at("p1")], &HashSet::new(), false, 0).is_empty());
        assert!(watch.judge(&[at("p1")], &HashSet::new(), false, 3 * HOUR_MS).is_empty());
        assert_eq!(told(&mut watch, &[at("p1")], 3 * HOUR_MS + 1), ["p1"]);
    }

    #[test]
    fn the_notice_names_the_pane_and_how_to_bring_it_forward() {
        let said = body("p1", Some("otter"), 2 * HOUR_MS);
        assert!(said.starts_with("otter (p1) has been at a dialog for 120 minutes"), "{said}");
        assert!(said.contains("`muster pane show --pane otter`"), "{said}");
    }
}
