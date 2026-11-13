//! A pane left at a dialog for hours is told to the person, once for each block. The stall watch
//! tells them of a dialog only when a message waits on it, and the maker watch only of a helper's,
//! so an agent that asked a question nobody is waiting on would otherwise sit until someone
//! happens to look: the one-time blocked banner is easy to miss, and two hours is longer than
//! anyone at the window leaves a dialog they can see.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use super::Block as Carried;

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
    /// Whether it was made quiet, which promises nobody is told of it.
    pub(crate) quiet: bool,
    /// Whether a message waits on it, which the stall watch tells of.
    pub(crate) unread: bool,
}

impl Blocked<'_> {
    /// Whether this watch times it: not a helper, which the maker watch tells of, nor a quiet
    /// pane, which nobody is told of.
    fn watched(&self) -> bool {
        !self.helper && !self.quiet
    }
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
    /// The dialogs the daemon that handed the panes over was timing, taken up by the first look
    /// that finds the pane still at one, and dropped by one that finds it at none.
    carried: HashMap<String, Carried>,
    /// What was last given the session to carry, by pane.
    kept: BTreeMap<String, Carried>,
}

impl LongBlocks {
    pub(crate) fn new() -> LongBlocks {
        LongBlocks::after(long_block())
    }

    fn after(limit: Duration) -> LongBlocks {
        LongBlocks {
            limit_ms: u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
            blocks: HashMap::new(),
            carried: HashMap::new(),
            kept: BTreeMap::new(),
        }
    }

    /// Takes the dialogs the daemon that handed the panes over was timing.
    pub(crate) fn carry(&mut self, blocks: &BTreeMap<String, Carried>) {
        self.carried.extend(blocks.iter().map(|(pane, block)| (pane.clone(), *block)));
    }

    /// Each pane whose dialog, or lack of one, changed since this was last asked, for a handoff
    /// to carry.
    pub(crate) fn changed_blocks(&mut self) -> Vec<(String, Option<Carried>)> {
        let now: BTreeMap<String, Carried> = self
            .blocks
            .iter()
            .map(|(pane, block)| {
                (pane.clone(), Carried { since_ms: block.since_ms, person_told: block.told })
            })
            .collect();
        let mut changed: Vec<(String, Option<Carried>)> = self
            .kept
            .keys()
            .filter(|pane| !now.contains_key(*pane))
            .map(|pane| (pane.clone(), None))
            .collect();
        changed.extend(
            now.iter()
                .filter(|(pane, block)| self.kept.get(*pane) != Some(block))
                .map(|(pane, block)| (pane.clone(), Some(*block))),
        );
        self.kept = now;
        changed
    }

    /// Weighs each pane at a dialog now and says which to tell the person of, with how long each
    /// has been there. `seen` holds every pane a look found an agent in, `escalated` the panes
    /// the stall watch told the person of at this look, which this block is then told of already.
    /// Pure, so every rule here is a unit test.
    pub(crate) fn judge(
        &mut self,
        blocked: &[Blocked<'_>],
        seen: &HashSet<&str>,
        escalated: &HashSet<&str>,
        in_reach: bool,
        now_ms: u64,
    ) -> Vec<(String, u64)> {
        // A pane no longer at a dialog has ended its block, and a later one is told afresh.
        self.blocks.retain(|pane, _| blocked.iter().any(|at| at.pane == pane && at.watched()));
        let at_a_dialog = |pane: &str| blocked.iter().any(|at| at.pane == pane);
        self.carried.retain(|pane, _| !seen.contains(pane.as_str()) || at_a_dialog(pane));
        let mut due = Vec::new();
        for at in blocked.iter().filter(|at| at.watched()) {
            let carried = &mut self.carried;
            let block = self.blocks.entry(at.pane.to_string()).or_insert_with(|| {
                carried.remove(at.pane).map_or(Block { since_ms: now_ms, told: false }, |was| {
                    Block { since_ms: was.since_ms, told: was.person_told }
                })
            });
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
        Blocked { pane, helper: false, quiet: false, unread: false }
    }

    fn told(watch: &mut LongBlocks, blocked: &[Blocked<'_>], now_ms: u64) -> Vec<String> {
        watch
            .judge(blocked, &HashSet::new(), &HashSet::new(), true, now_ms)
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
    fn a_helper_a_quiet_pane_or_a_dialog_a_message_waits_on_is_not_told() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        let helper = Blocked { helper: true, ..at("p2") };
        let quiet = Blocked { quiet: true, ..at("p4") };
        let unread = Blocked { unread: true, ..at("p3") };
        told(&mut watch, &[helper, unread, quiet], 0);
        assert!(told(&mut watch, &[helper, unread, quiet], 3 * HOUR_MS).is_empty());
        assert_eq!(told(&mut watch, &[helper, at("p3")], 3 * HOUR_MS + 1), ["p3"], "read since");
    }

    #[test]
    fn a_block_the_stall_watch_told_the_person_of_is_not_told_again() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        let unread = Blocked { unread: true, ..at("p1") };
        watch.judge(&[unread], &HashSet::new(), &HashSet::new(), true, 0);
        watch.judge(&[unread], &HashSet::new(), &HashSet::from(["p1"]), true, HOUR_MS / 2);
        assert!(told(&mut watch, &[at("p1")], 3 * HOUR_MS).is_empty());
    }

    #[test]
    fn a_block_waits_for_the_person_to_be_in_reach() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        assert!(watch.judge(&[at("p1")], &HashSet::new(), &HashSet::new(), false, 0).is_empty());
        assert!(
            watch
                .judge(&[at("p1")], &HashSet::new(), &HashSet::new(), false, 3 * HOUR_MS)
                .is_empty()
        );
        assert_eq!(told(&mut watch, &[at("p1")], 3 * HOUR_MS + 1), ["p1"]);
    }

    /// A block the daemon before this one was timing goes on from when it began, and one the
    /// person was told of is not told again. One for a pane found at no dialog is dropped.
    #[test]
    fn a_carried_block_keeps_its_clock_and_whether_the_person_was_told() {
        let mut watch = LongBlocks::after(LONG_BLOCK);
        watch.carry(&BTreeMap::from([
            ("p1".to_string(), Carried { since_ms: 0, person_told: false }),
            ("p2".to_string(), Carried { since_ms: 0, person_told: true }),
            ("p3".to_string(), Carried { since_ms: 0, person_told: false }),
        ]));
        let seen = HashSet::from(["p1", "p2", "p3"]);
        let blocked = [at("p1"), at("p2")];
        let told = watch.judge(&blocked, &seen, &HashSet::new(), true, 2 * HOUR_MS);
        assert_eq!(told.into_iter().map(|(pane, _)| pane).collect::<Vec<_>>(), ["p1"]);
        assert!(told_after(&mut watch, &[at("p1"), at("p2"), at("p3")], 3 * HOUR_MS).is_empty());
        assert_eq!(told_after(&mut watch, &[at("p3")], 5 * HOUR_MS), ["p3"], "p3 began anew");
    }

    fn told_after(watch: &mut LongBlocks, blocked: &[Blocked<'_>], now_ms: u64) -> Vec<String> {
        told(watch, blocked, now_ms)
    }

    #[test]
    fn the_notice_names_the_pane_and_how_to_bring_it_forward() {
        let said = body("p1", Some("otter"), 2 * HOUR_MS);
        assert!(said.starts_with("otter (p1) has been at a dialog for 120 minutes"), "{said}");
        assert!(said.contains("`muster pane show --pane otter`"), "{said}");
    }
}
