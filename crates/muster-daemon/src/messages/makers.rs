//! A helper that needs somebody tells its maker (MIP-7, section 4): one that becomes blocked,
//! becomes stuck, or goes idle without having posted to its maker since it last went to work.
//!
//! A helper is quiet, so none of that reaches the person; its maker made it and is the agent
//! that can act on it. The helper's daemon watches its own helpers, since only it can see their
//! panes, and tells the maker once each time, in a message from Muster (`@muster`) in the group
//! the two share, in the stall watch's words (`super::stalls`). A helper whose maker is on
//! another machine is not watched yet.
//!
//! A need is told once it has held for [`SETTLE`]: a dialog the person answers at once, or a turn
//! the maker's next message starts again straight away, wakes nobody.
//!
//! When a maker's pane closes, its helpers go on running and stop being quiet, which the session
//! does as the pane closes, and the person is told here, once per maker (MIP-7, section 6).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_msg::{Activity, Presence};

use super::presence::Panes;
use super::stalls::{AT_A_DIALOG, said_as_time, shown, stopped_reading};
use super::{delivering, now_ms, told};
use crate::session::{Orphaned, Shared};

/// How long a helper's need must hold before its maker is told of it.
pub(crate) const SETTLE: Duration = Duration::from_secs(10);

/// A debug build's [`SETTLE`] in milliseconds, when set, so a test need not wait for it.
const SETTLE_SAID: &str = "MUSTER_DAEMON_HELPER_SETTLE_MS";

/// How long the doorbell's thread may sleep for all this watch cares, with no need waiting to
/// be told: every change in an agent wakes it anyway.
const NOTHING_DUE: Duration = Duration::from_mins(1);

fn settle() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(SETTLE_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    SETTLE
}

/// A helper with an agent in it, whose maker is a pane here, as this daemon sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Helper {
    pub(crate) pane: String,
    pub(crate) maker: String,
    pub(crate) activity: Option<Activity>,
    /// The program in its pane that has stopped reading, and what it left unread.
    pub(crate) stopped: Option<(String, u32)>,
    /// When it last posted to its maker, looked up only while it is idle.
    pub(crate) last_post_ms: Option<u64>,
}

/// What a helper needs that its maker is told of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Blocked,
    Stuck,
    /// Idle without having posted to its maker since it last went to work.
    Unreported,
}

impl Need {
    fn as_str(self) -> &'static str {
        match self {
            Need::Blocked => "blocked",
            Need::Stuck => "stuck",
            Need::Unreported => "unreported",
        }
    }
}

/// What a look decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tell {
    pub(crate) maker: String,
    pub(crate) helper: String,
    pub(crate) need: Need,
    pub(crate) body: String,
}

/// One helper, between looks.
#[derive(Debug, Default)]
struct Watch {
    /// When its current stretch of work began: from the look that first saw it working or
    /// waiting on its own work, through any dialog, to the look that sees it idle.
    worked_since_ms: Option<u64>,
    at_work: bool,
    /// What it needs, and since when.
    need: Option<(Need, u64)>,
    told: bool,
}

/// Every helper this daemon is watching, between looks.
#[derive(Debug)]
pub(crate) struct Makers {
    settle_ms: u64,
    watches: HashMap<String, Watch>,
    /// Helpers whose maker is on another machine, said once each.
    elsewhere: HashSet<String>,
}

impl Makers {
    pub(crate) fn new() -> Makers {
        Makers::after(settle())
    }

    fn after(settle: Duration) -> Makers {
        Makers {
            settle_ms: u64::try_from(settle.as_millis()).unwrap_or(u64::MAX),
            watches: HashMap::new(),
            elsewhere: HashSet::new(),
        }
    }

    /// Weighs what each of `helpers` needs now and says whom to tell; `called` is what each
    /// pane is called, by id, to say them by. Pure, so every rule here is a unit test.
    pub(crate) fn judge(
        &mut self,
        helpers: &[Helper],
        called: &BTreeMap<String, String>,
        now_ms: u64,
    ) -> Vec<Tell> {
        self.watches.retain(|pane, _| helpers.iter().any(|helper| helper.pane == *pane));
        let mut tells = Vec::new();
        for helper in helpers {
            let watch = self.watches.entry(helper.pane.clone()).or_default();
            match helper.activity {
                Some(Activity::Working | Activity::Waiting) => {
                    if !watch.at_work {
                        watch.worked_since_ms = Some(now_ms);
                    }
                    watch.at_work = true;
                }
                Some(Activity::Idle) => watch.at_work = false,
                Some(Activity::Blocked | Activity::Stuck) | None => {}
            }
            let need = need(helper, watch.worked_since_ms);
            if watch.need.map(|(was, _)| was) != need {
                watch.need = need.map(|need| (need, now_ms));
                watch.told = false;
            }
            let Some((need, since_ms)) = watch.need else { continue };
            if watch.told || now_ms.saturating_sub(since_ms) < self.settle_ms {
                continue;
            }
            watch.told = true;
            let worked_ms = now_ms.saturating_sub(watch.worked_since_ms.unwrap_or(now_ms));
            tells.push(Tell {
                maker: helper.maker.clone(),
                helper: helper.pane.clone(),
                need,
                body: body(helper, need, called, worked_ms),
            });
        }
        tells
    }

    /// How long until a need held now is due to be told.
    fn due(&self, now_ms: u64) -> Duration {
        self.watches
            .values()
            .filter(|watch| !watch.told)
            .filter_map(|watch| watch.need)
            .map(|(_, since_ms)| {
                let left = (since_ms + self.settle_ms).saturating_sub(now_ms);
                Duration::from_millis(left)
            })
            .min()
            .unwrap_or(NOTHING_DUE)
    }
}

/// What `helper` needs now, `worked_since_ms` being when its last stretch of work began.
fn need(helper: &Helper, worked_since_ms: Option<u64>) -> Option<Need> {
    if helper.stopped.is_some() || helper.activity == Some(Activity::Stuck) {
        return Some(Need::Stuck);
    }
    match helper.activity {
        Some(Activity::Blocked) => Some(Need::Blocked),
        Some(Activity::Idle) => {
            let worked = worked_since_ms?;
            let posted = helper.last_post_ms.is_some_and(|posted| posted >= worked);
            (!posted).then_some(Need::Unreported)
        }
        _ => None,
    }
}

/// What the maker is told: the MIP's own sentence, the stall watch's reason, and what the maker
/// can do about it.
fn body(helper: &Helper, need: Need, called: &BTreeMap<String, String>, worked_ms: u64) -> String {
    let named = shown(called, &helper.pane);
    // What a command takes it by: its name, which `--pane` takes as it takes the id.
    let pane = called.get(&helper.pane).unwrap_or(&helper.pane);
    match need {
        Need::Blocked => format!(
            "{named} is blocked at a dialog: {AT_A_DIALOG}. `muster pane read --pane {pane}` \
             shows it and `muster pane send --pane {pane} --key <key>` answers it, or bring it \
             forward with `muster pane show --pane {pane}` and ask the person. Muster says this \
             once each time it blocks."
        ),
        Need::Stuck => {
            let why = helper.stopped.as_ref().map_or_else(
                || "nothing reaches it until somebody interrupts it".to_string(),
                |(program, bytes)| stopped_reading(program, *bytes),
            );
            format!(
                "{named} has stopped reading its input: {why}. `muster pane restart --pane \
                 {pane}` ends it and resumes its session. Muster says this once each time it \
                 stops."
            )
        }
        Need::Unreported => format!(
            "{named} went idle without posting to you since it went to work {worked} ago: its \
             turn may have ended on an error, or it stopped without saying so. `muster pane read \
             --pane {pane} --turn` shows what it printed. Muster says this once each time it goes \
             idle.",
            worked = said_as_time(worked_ms),
        ),
    }
}

/// Looks at each helper here, and tells its maker what it needs. Called by the doorbell's thread
/// with no lock held, on every wake; says how long until the next look is due.
pub(crate) fn look(shared: &Arc<Shared>, makers: &mut Makers) -> Duration {
    let (orphans, helping) = {
        let mut session = shared.lock();
        (session.take_orphans(), session.has_helpers())
    };
    if orphans.is_empty() && !helping {
        makers.watches.clear();
        return NOTHING_DUE;
    }
    let panes = Panes::of(shared);
    for orphaned in &orphans {
        tell_person(shared, &panes, orphaned);
    }
    let (helpers, called) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return NOTHING_DUE;
        }
        let helpers = helpers(&messages.service, &panes, &mut makers.elsewhere);
        (helpers, messages.service.called(&panes))
    };
    let mut called = called;
    for helper in &helpers {
        for pane in [&helper.pane, &helper.maker] {
            if let Some(name) = panes.name_of(pane) {
                called.insert(pane.clone(), name);
            }
        }
    }
    let now = now_ms();
    for tell in makers.judge(&helpers, &called, now) {
        tell_maker(shared, &panes, &tell);
    }
    makers.due(now_ms())
}

/// Every helper here with an agent in it whose maker is a pane here.
fn helpers(
    service: &muster_msg::Messaging<super::store::Files>,
    panes: &Panes,
    elsewhere: &mut HashSet<String>,
) -> Vec<Helper> {
    let mut helpers = Vec::new();
    for (pane, maker) in panes.makers() {
        if !panes.exists(maker) {
            if elsewhere.insert(pane.clone()) {
                log::debug(
                    "msg.helper.maker_elsewhere",
                    fields! { "helper" => pane, "maker" => maker },
                );
            }
            continue;
        }
        let Some(seen) = panes.get(pane) else { continue };
        let idle = seen.activity == Some(Activity::Idle);
        helpers.push(Helper {
            pane: pane.clone(),
            maker: maker.clone(),
            activity: seen.activity,
            stopped: seen.stopped.clone(),
            last_post_ms: if idle { service.last_post(pane, maker) } else { None },
        });
    }
    helpers
}

/// What the person is told of a maker that closed with its helpers still running.
///
/// The command names the maker by its id, which its helpers still carry: its name went with it.
fn orphaned_body(orphaned: &Orphaned) -> String {
    let (maker, id) = (&orphaned.called, &orphaned.maker);
    match orphaned.helpers.as_slice() {
        [only] => format!(
            "{maker} closed; its helper {only} is still running, and is no longer quiet. `muster \
             pane show --helpers-of {id}` brings it forward."
        ),
        helpers => format!(
            "{maker} closed; {count} of its helpers are still running, and are no longer quiet: \
             {names}. `muster pane show --helpers-of {id}` brings them forward.",
            count = helpers.len(),
            names = helpers.join(", "),
        ),
    }
}

fn tell_person(shared: &Arc<Shared>, panes: &Panes, orphaned: &Orphaned) {
    let body = orphaned_body(orphaned);
    let delivered = delivering(shared, panes, |service| {
        let called = Some(orphaned.called.as_str());
        service.tell_as_muster(muster_msg::HUMAN, &orphaned.maker, called, &body, panes, now_ms())
    });
    match delivered {
        Ok(delivered) => {
            told("msg.helper.orphans_told", Some(body.len()), &delivered);
        }
        Err(reply) => log::warn(
            "msg.helper.orphans_untold",
            fields! {
                "maker" => orphaned.maker,
                "helpers" => orphaned.helpers.join(","),
                "reason" => reply.reason,
                "impact" => "the person was not told that these helpers are still running with \
                             nobody watching them; they are no longer quiet, so their next block \
                             or finish notifies as any pane's does",
                "check" => "the reason: the person homed on another machine, where Muster \
                            cannot tell them from here, or a store that would not write",
            },
        ),
    }
}

fn tell_maker(shared: &Arc<Shared>, panes: &Panes, tell: &Tell) {
    log::info(
        "msg.helper.needs",
        fields! { "helper" => tell.helper, "maker" => tell.maker, "need" => tell.need.as_str() },
    );
    let delivered = delivering(shared, panes, |service| {
        service.tell_as_muster(&tell.maker, &tell.helper, None, &tell.body, panes, now_ms())
    });
    match delivered {
        Ok(delivered) => {
            told("msg.helper.told", Some(tell.body.len()), &delivered);
        }
        Err(reply) => log::warn(
            "msg.helper.untold",
            fields! {
                "helper" => tell.helper,
                "maker" => tell.maker,
                "need" => tell.need.as_str(),
                "reason" => reply.reason,
                "impact" => "the helper is quiet, so nobody hears that it needs somebody: its \
                             maker was not told, and the person is not told while it is quiet",
                "check" => "the reason: a group the two share kept on another machine, or a \
                            store that would not write; `muster pane quiet --pane <helper> \
                            --off` makes it ask the person instead",
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLE_MS: u64 = 10_000;

    fn makers() -> Makers {
        Makers::after(Duration::from_millis(SETTLE_MS))
    }

    /// `maple` in `p2`, made by `p1`, in `activity`.
    fn maple(activity: Activity) -> Helper {
        Helper {
            pane: "p2".to_string(),
            maker: "p1".to_string(),
            activity: Some(activity),
            stopped: None,
            last_post_ms: None,
        }
    }

    fn called() -> BTreeMap<String, String> {
        BTreeMap::from([("p2".to_string(), "maple".to_string())])
    }

    fn needs(tells: &[Tell]) -> Vec<Need> {
        tells.iter().map(|tell| tell.need).collect()
    }

    /// A block is told once it has held for the settle, once; one that clears sooner is not;
    /// and blocking again after going back to work is told again.
    #[test]
    fn a_block_is_told_once_after_the_settle_and_again_each_time() {
        let mut makers = makers();
        let called = called();
        assert!(makers.judge(&[maple(Activity::Working)], &called, 0).is_empty());
        assert!(makers.judge(&[maple(Activity::Blocked)], &called, 1_000).is_empty());
        assert!(makers.judge(&[maple(Activity::Working)], &called, 5_000).is_empty(), "answered");
        assert!(makers.judge(&[maple(Activity::Working)], &called, 20_000).is_empty());

        assert!(makers.judge(&[maple(Activity::Blocked)], &called, 21_000).is_empty());
        let tells = makers.judge(&[maple(Activity::Blocked)], &called, 31_000);
        assert_eq!(needs(&tells), [Need::Blocked]);
        assert_eq!(tells[0].maker, "p1");
        assert!(tells[0].body.starts_with("maple (p2) is blocked at a dialog: it is at a dialog"));
        assert!(tells[0].body.contains("--pane maple"), "{}", tells[0].body);
        assert!(makers.judge(&[maple(Activity::Blocked)], &called, 60_000).is_empty(), "once");

        makers.judge(&[maple(Activity::Working)], &called, 61_000);
        makers.judge(&[maple(Activity::Blocked)], &called, 62_000);
        let again = makers.judge(&[maple(Activity::Blocked)], &called, 72_000);
        assert_eq!(needs(&again), [Need::Blocked], "blocked again");
    }

    /// A helper whose program stopped reading is told of, with the stall watch's reason.
    #[test]
    fn a_helper_that_stopped_reading_is_told_of() {
        let mut makers = makers();
        let stuck = Helper { stopped: Some(("claude".to_string(), 812)), ..maple(Activity::Stuck) };
        assert!(makers.judge(std::slice::from_ref(&stuck), &called(), 0).is_empty());
        let tells = makers.judge(&[stuck], &called(), SETTLE_MS);
        assert_eq!(needs(&tells), [Need::Stuck]);
        assert!(tells[0].body.contains("its program, claude, has stopped reading its input"));
    }

    /// Idle after work with nothing posted to its maker since is told; idle after posting is
    /// not; nor is an idle never seen at work, nor waiting on its own work.
    #[test]
    fn idle_without_a_report_since_work_is_told_and_after_one_is_not() {
        let called = called();
        let mut unreported = makers();
        unreported.judge(&[maple(Activity::Idle)], &called, 0);
        assert!(
            unreported.judge(&[maple(Activity::Idle)], &called, 60_000).is_empty(),
            "never worked"
        );

        unreported.judge(&[maple(Activity::Working)], &called, 100_000);
        unreported.judge(&[maple(Activity::Waiting)], &called, 200_000);
        assert!(
            unreported.judge(&[maple(Activity::Waiting)], &called, 300_000).is_empty(),
            "waiting"
        );
        unreported.judge(&[maple(Activity::Idle)], &called, 400_000);
        let tells = unreported.judge(&[maple(Activity::Idle)], &called, 410_000);
        assert_eq!(needs(&tells), [Need::Unreported]);
        assert!(tells[0].body.contains("went idle without posting to you since it went to work 5"));

        let mut reporting = makers();
        reporting.judge(&[maple(Activity::Working)], &called, 0);
        let reported = Helper { last_post_ms: Some(50_000), ..maple(Activity::Idle) };
        reporting.judge(std::slice::from_ref(&reported), &called, 60_000);
        assert!(reporting.judge(&[reported], &called, 90_000).is_empty(), "it posted its report");
    }

    /// A dialog in the middle of a turn does not start the turn again: a report posted before
    /// it still counts once the helper goes idle after it.
    #[test]
    fn a_report_before_a_dialog_still_counts_once_the_turn_ends() {
        let called = called();
        let mut makers = makers();
        makers.judge(&[maple(Activity::Working)], &called, 0);
        makers.judge(&[maple(Activity::Blocked)], &called, 10_000);
        makers.judge(&[maple(Activity::Working)], &called, 12_000);
        let reported = Helper { last_post_ms: Some(5_000), ..maple(Activity::Idle) };
        makers.judge(std::slice::from_ref(&reported), &called, 20_000);
        assert!(makers.judge(&[reported], &called, 40_000).is_empty());
    }

    /// The person is told every helper a closed maker left running, and how to bring them back.
    #[test]
    fn a_closed_maker_is_said_with_every_helper_it_left_running() {
        let orphaned = |helpers: &[&str]| Orphaned {
            maker: "p1".to_string(),
            called: "🤖 integrator".to_string(),
            helpers: helpers.iter().map(|helper| (*helper).to_string()).collect(),
        };
        assert_eq!(
            orphaned_body(&orphaned(&["🤖 1", "maple", "cobalt"])),
            "🤖 integrator closed; 3 of its helpers are still running, and are no longer quiet: \
             🤖 1, maple, cobalt. `muster pane show --helpers-of p1` brings them forward."
        );
        assert!(orphaned_body(&orphaned(&["🤖 1"])).starts_with(
            "🤖 integrator closed; its helper 🤖 1 is still running, and is no longer quiet."
        ));
    }

    /// The next look is due when the oldest untold need reaches the settle.
    #[test]
    fn the_next_look_is_due_when_a_need_settles() {
        let mut makers = makers();
        assert_eq!(makers.due(0), NOTHING_DUE);
        makers.judge(&[maple(Activity::Blocked)], &called(), 1_000);
        assert_eq!(makers.due(4_000), Duration::from_secs(7));
    }
}
