//! A helper that needs somebody tells its maker (MIP-7, section 4): one that becomes blocked,
//! becomes stuck, or goes idle without having posted to its maker since it last went to work.
//!
//! A helper is quiet, so none of that reaches the person; its maker made it and is the agent
//! that can act on it. The helper's daemon watches its own helpers, since only it can see their
//! panes, and tells the maker once each time, in a message from Muster (`@muster`) in the group
//! the two share, in the stall watch's words (`super::stalls`). A maker on another machine is
//! reached as MIP-4 section 11 reaches a member there: that machine says which participant the
//! maker's pane is, over a link only a window attached to both makes, and the notice goes in a
//! group kept here, which that machine is sent.
//!
//! A need is told once it has held for [`SETTLE`]: a dialog the person answers at once, or a turn
//! the maker's next message starts again straight away, wakes nobody. And it is told only once
//! its maker can hear it: a post to a maker at work waits for its turn to end, and a need held
//! that long has often been met, as a plan dialog the maker itself is about to answer is. So
//! while the maker is busy the need is held, and told when it comes free if it still holds.
//!
//! When a maker's pane closes, its helpers stop being quiet, which the session does as the pane
//! closes, and each is closed here once it has been at rest for [`SETTLE`]: its agent idle, or no
//! agent in it. One at work runs on until then, and the person is told of those, once per maker.
//! On a machine the person is not homed on, that waits until their machine is linked to this
//! one: a group made here while no link is up would never be sent there.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::names::{Mint, Minter};
use muster_daemon_proto as proto;
use muster_msg::{Activity, Presence, Reach};

use super::busy::{ASK_BUSY_AGAIN, Busy};
use super::presence::Panes;
use super::stalls::{AT_A_DIALOG, said_as_time, shown, stopped_reading};
use super::{delivering, notices, now_ms, told, waits, words};
use crate::pane::TypedBy;
use crate::session::{Orphaned, Session, Shared};

/// How long a helper's need must hold before its maker is told of it.
pub(crate) const SETTLE: Duration = Duration::from_secs(10);

/// A debug build's [`SETTLE`] in milliseconds, when set, so a test need not wait for it.
const SETTLE_SAID: &str = "MUSTER_DAEMON_HELPER_SETTLE_MS";

/// How many times [`SETTLE`] a helper may sit at a dialog before the person is told, whatever
/// its maker did: 30 minutes, the span the stall watch waits before telling the person of a
/// member only a person can free (`super::stalls`).
const ESCALATE_AFTER_SETTLES: u64 = 180;

/// How long the doorbell's thread may sleep for all this watch cares, with no need waiting to
/// be told: every change in an agent wakes it anyway.
const NOTHING_DUE: Duration = Duration::from_mins(1);

/// How long a maker no linked machine had is left before they are asked again.
const ASK_AGAIN: Duration = Duration::from_mins(1);

fn waiting_limit() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var("MUSTER_DAEMON_HELPER_WAIT_MS")
        && let Ok(ms) = said.parse::<u64>()
    {
        return Duration::from_millis(ms);
    }
    Duration::from_mins(10)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub(crate) reason: String,
    pub(crate) since_ms: u64,
    pub(crate) noticed: bool,
}

fn settle() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(SETTLE_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    SETTLE
}

/// A helper with an agent in it, whose maker is a pane here or one another machine has, as this
/// daemon sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent facts about a helper, one each")]
pub(crate) struct Helper {
    pub(crate) pane: String,
    pub(crate) maker: String,
    /// Whom Muster tells: the maker's pane here, or the participant another machine said it is,
    /// as this machine writes them (`claude@lap`).
    pub(crate) to: String,
    pub(crate) activity: Option<Activity>,
    /// Whether its agent's turn, at work now or ended last, is one Muster typed as a chore of its
    /// own: a compaction or a rename, which is not the helper going to work.
    pub(crate) chores_turn: bool,
    /// Which of its agent's turns that is, counted since the daemon started: a turn can be found to
    /// be a chore after it was taken for work.
    pub(crate) turn: u64,
    /// What the wait it declared was on, when that wait ran out with nothing ending it and it
    /// has not gone to work since.
    pub(crate) ran_out: Option<String>,
    pub(crate) waiting: Option<Waiting>,
    /// The program in its pane that has stopped reading, and what it left unread.
    pub(crate) stopped: Option<(String, u32)>,
    /// When it last posted to anybody, looked up only while it is idle: a post to its maker, to
    /// a helper of its own or to another agent all say it ended its turn on purpose.
    pub(crate) last_post_ms: Option<u64>,
    /// Whether a helper of its own is at work, waiting or at a dialog, which it is waiting on.
    pub(crate) helpers_at_work: bool,
    /// Whether something new asks it for work: somebody typed what started its turn, or a
    /// message for it from somebody other than Muster came after its last post. Looked up only
    /// while it is at work or waiting.
    pub(crate) asked: bool,
    /// Whether its maker would not be woken by a post now: an agent here at work, at a dialog or
    /// stuck, whose hooks do not fetch messages mid-turn; or one on another machine its machine
    /// last said was any of those, hooks or not, since the link does not say.
    pub(crate) maker_busy: bool,
    /// Whether its maker's pane is on this machine, which wakes this watch when the maker comes
    /// free; one elsewhere is asked again every [`ASK_BUSY_AGAIN`].
    pub(crate) maker_here: bool,
}

/// What a helper needs that its maker is told of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Blocked,
    Stuck,
    /// Idle without having posted to its maker since it last went to work.
    Unreported,
    Waiting,
}

impl Need {
    fn as_str(self) -> &'static str {
        match self {
            Need::Blocked => "blocked",
            Need::Stuck => "stuck",
            Need::Unreported => "unreported",
            Need::Waiting => "waiting",
        }
    }
}

/// What a look decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tell {
    pub(crate) maker: String,
    pub(crate) to: String,
    pub(crate) helper: String,
    pub(crate) need: Need,
    pub(crate) body: String,
    wait_started_ms: Option<u64>,
}

/// One helper, between looks.
#[derive(Debug, Default)]
struct Watch {
    /// When its current stretch of work began: from the look that first saw it working or
    /// waiting on its own work, through any dialog, to the look that sees it idle.
    worked_since_ms: Option<u64>,
    at_work: bool,
    /// The agent's turn its current stretch began on, and when the stretch before it began: what a
    /// stretch found to have begun on a chore goes back to.
    stretch_turn: Option<u64>,
    worked_before_ms: Option<u64>,
    /// When a look first saw it at a dialog while it was not at work: where its stretch began if
    /// it goes to work next, which a look the load delayed can see after the dialog. One that
    /// goes idle next was a dialog at its idle prompt, a folder's trust or a picker, and no work.
    dialog_since_ms: Option<u64>,
    /// What it needs, and since when.
    need: Option<(Need, u64)>,
    told: bool,
    /// The stretch of work, by when it began, that its maker was told it went idle unreported
    /// after: a chore turn between leaves the same stretch, which is not told of twice.
    told_unreported_for: Option<u64>,
    /// When the need began that the person has been told of, which its maker had not met: a need
    /// begun later is another, which they hear of afresh.
    escalated_for: Option<u64>,
    /// Whether its need is due and waits for its maker to be free.
    held: bool,
    waiting_since_ms: Option<u64>,
    told_wait_since_ms: Option<u64>,
    retry_at_ms: Option<u64>,
}

impl Watch {
    fn update_need(&mut self, helper: &Helper, now_ms: u64) {
        let waiting_since = helper.waiting.as_ref().map(|waiting| waiting.since_ms);
        if self.waiting_since_ms != waiting_since {
            if waiting_since.is_some() {
                self.told_wait_since_ms = None;
            }
            if self.need.is_some_and(|(need, _)| need == Need::Waiting) {
                self.need = None;
                self.told = false;
                self.held = false;
                self.retry_at_ms = None;
            }
            self.waiting_since_ms = waiting_since;
        }
        let need = need(helper, self.worked_since_ms);
        if self.need.map(|(was, _)| was) != need {
            self.need = need.map(|need| {
                (need, if need == Need::Waiting { waiting_since.unwrap_or(now_ms) } else { now_ms })
            });
            self.told = need == Some(Need::Unreported)
                && self.worked_since_ms.is_some()
                && self.told_unreported_for == self.worked_since_ms;
            if need == Some(Need::Waiting) {
                self.told = helper.waiting.as_ref().is_some_and(|waiting| waiting.noticed)
                    || (waiting_since.is_some() && self.told_wait_since_ms == waiting_since);
                if self.told {
                    self.told_wait_since_ms = waiting_since;
                }
            }
            if need == Some(Need::Unreported)
                && helper.ran_out.is_some()
                && self.told_wait_since_ms.is_some()
            {
                self.told = true;
            }
            self.held = false;
            self.retry_at_ms = None;
        }
    }
}

/// Every helper this daemon is watching, between looks.
#[derive(Debug)]
pub(crate) struct Makers {
    settle_ms: u64,
    waiting_ms: u64,
    watches: HashMap<String, Watch>,
    /// Helpers whose maker no machine linked to this one has, said once each.
    elsewhere: HashSet<String>,
    /// The participant each maker on another machine is, as that machine said, by the maker.
    there: HashMap<String, String>,
    /// When each maker no linked machine had was last asked about, by the maker.
    asked_ms: HashMap<String, u64>,
    /// Makers that closed whose notice waits for the person's machine to be linked to this one.
    held: Vec<Orphaned>,
    /// Makers that closed, and when this watch heard: the person is told of the helpers still
    /// running once the settle has passed, so of none that closed with their maker.
    unsettled: Vec<(Orphaned, u64)>,
    /// Since when each helper whose maker has closed has been at rest, by the helper.
    at_rest_since_ms: HashMap<String, u64>,
    /// Whether a helper whose maker has closed holds no agent yet and may still get one: nothing
    /// wakes the doorbell's thread when that time is up, so it looks again after a settle.
    agent_awaited: bool,
    /// What each helper's maker has been told it needs, as the session last heard, so that a
    /// daemon taking the panes over does not tell them again.
    told: BTreeMap<String, Need>,
    /// What the daemon that handed these panes over had told each helper's maker, by the helper,
    /// taken as told when the helper is first weighed here.
    carried: BTreeMap<String, Need>,
    /// Each need held since the last look, for it to log: the helper, its maker, and the need.
    newly_held: Vec<(String, String, Need)>,
    /// What the machines of participants elsewhere last said of whether each is busy, which the
    /// stall watch asks too.
    pub(crate) busy: Busy,
    /// Whether a need waits for a maker on another machine to come free, which nothing announces.
    held_elsewhere: bool,
    escalate_ms: u64,
    /// Whether what is posted to the person here reaches them now, as of this look.
    human_in_reach: bool,
    /// Each helper whose dialog its maker has left for the person, since the last look, with what
    /// the person is told.
    escalations: Vec<(String, String)>,
}

impl Makers {
    pub(crate) fn new() -> Makers {
        Makers::after(settle())
    }

    fn after(settle: Duration) -> Makers {
        Makers {
            settle_ms: u64::try_from(settle.as_millis()).unwrap_or(u64::MAX),
            waiting_ms: u64::try_from(waiting_limit().as_millis()).unwrap_or(u64::MAX),
            watches: HashMap::new(),
            elsewhere: HashSet::new(),
            there: HashMap::new(),
            asked_ms: HashMap::new(),
            held: Vec::new(),
            unsettled: Vec::new(),
            at_rest_since_ms: HashMap::new(),
            agent_awaited: false,
            told: BTreeMap::new(),
            carried: BTreeMap::new(),
            newly_held: Vec::new(),
            busy: Busy::default(),
            held_elsewhere: false,
            escalate_ms: u64::try_from(settle.as_millis())
                .unwrap_or(u64::MAX)
                .saturating_mul(ESCALATE_AFTER_SETTLES),
            human_in_reach: true,
            escalations: Vec::new(),
        }
    }

    /// What each helper's maker has been told it needs and it still needs.
    fn telling(&self) -> BTreeMap<String, Need> {
        let told = self.watches.iter().filter(|(_, watch)| watch.told);
        told.filter_map(|(pane, watch)| Some((pane.clone(), watch.need?.0))).collect()
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
        self.held_elsewhere = false;
        let mut tells = Vec::new();
        for helper in helpers {
            let watch = self.watches.entry(helper.pane.clone()).or_insert_with(|| {
                let carried = self.carried.remove(&helper.pane);
                Watch {
                    need: carried.map(|need| (need, now_ms)),
                    told: carried.is_some(),
                    ..Watch::default()
                }
            });
            match helper.activity {
                Some(Activity::Working) if helper.chores_turn => {
                    // Found a chore after a look took it for work, as a compaction the harness
                    // started by itself is when the screen shows it before the harness says so.
                    if watch.at_work && watch.stretch_turn == Some(helper.turn) {
                        watch.at_work = false;
                        watch.worked_since_ms = watch.worked_before_ms;
                    }
                }
                // A turn nothing new started is no work: a wake for what it already read, a
                // handover, Muster's notice, a redraw the screen took for a turn. Its first work
                // counts whatever started it, as a prompt on its command line does.
                Some(Activity::Working | Activity::Waiting)
                    if !watch.at_work && !helper.asked && watch.worked_since_ms.is_some() => {}
                Some(Activity::Working | Activity::Waiting) => {
                    if !watch.at_work {
                        watch.worked_before_ms = watch.worked_since_ms;
                        watch.worked_since_ms = Some(watch.dialog_since_ms.unwrap_or(now_ms));
                        watch.stretch_turn = Some(helper.turn);
                    }
                    watch.at_work = true;
                    watch.dialog_since_ms = None;
                }
                Some(Activity::Blocked) if !watch.at_work => {
                    watch.dialog_since_ms.get_or_insert(now_ms);
                }
                Some(Activity::Idle) => {
                    watch.at_work = false;
                    watch.dialog_since_ms = None;
                }
                Some(Activity::Blocked | Activity::Stuck) | None => {}
            }
            watch.update_need(helper, now_ms);
            let waiting_since = helper.waiting.as_ref().map(|waiting| waiting.since_ms);
            let Some((need, since_ms)) = watch.need else { continue };
            let held_ms = now_ms.saturating_sub(since_ms);
            // Whether its maker was told or is still busy: only somebody at the pane can answer a
            // dialog, and half an hour on means the maker has not.
            if need == Need::Blocked
                && held_ms >= self.escalate_ms
                && watch.escalated_for != Some(since_ms)
                && self.human_in_reach
            {
                watch.escalated_for = Some(since_ms);
                self.escalations.push((helper.pane.clone(), escalation(helper, called, held_ms)));
            }
            let limit = if need == Need::Waiting { self.waiting_ms } else { self.settle_ms };
            if watch.told || held_ms < limit || watch.retry_at_ms.is_some_and(|at| now_ms < at) {
                continue;
            }
            if helper.maker_busy {
                if !watch.held {
                    watch.held = true;
                    self.newly_held.push((helper.pane.clone(), helper.maker.clone(), need));
                }
                self.held_elsewhere |= !helper.maker_here;
                continue;
            }
            watch.told = true;
            watch.held = false;
            if need == Need::Unreported {
                watch.told_unreported_for = watch.worked_since_ms;
            }
            if need == Need::Waiting {
                watch.told_wait_since_ms = waiting_since;
            }
            let worked_ms = now_ms.saturating_sub(watch.worked_since_ms.unwrap_or(now_ms));
            tells.push(Tell {
                maker: helper.maker.clone(),
                to: helper.to.clone(),
                helper: helper.pane.clone(),
                need,
                body: body(helper, need, called, worked_ms, held_ms, now_ms),
                wait_started_ms: (need == Need::Waiting).then_some(waiting_since).flatten(),
            });
        }
        tells
    }

    /// Which of `closing`, each helper whose maker has closed with whether it is at rest now,
    /// have been at rest for the settle, and so close. Pure, as [`Makers::judge`] is.
    fn settled<'a>(
        &mut self,
        closing: impl Iterator<Item = (&'a String, bool)>,
        now_ms: u64,
    ) -> Vec<String> {
        let at_rest: HashSet<&String> =
            closing.filter(|(_, at_rest)| *at_rest).map(|(pane, _)| pane).collect();
        self.at_rest_since_ms.retain(|pane, _| at_rest.contains(pane));
        let mut settled = Vec::new();
        for pane in at_rest {
            let since_ms = *self.at_rest_since_ms.entry(pane.clone()).or_insert(now_ms);
            if now_ms.saturating_sub(since_ms) >= self.settle_ms {
                settled.push(pane.clone());
            }
        }
        settled.sort();
        settled
    }

    /// The makers whose close this watch heard of a settle ago, whose helpers still running the
    /// person is now told of.
    fn settled_notices(&mut self, now_ms: u64) -> Vec<Orphaned> {
        let settle_ms = self.settle_ms;
        let (due, later) = std::mem::take(&mut self.unsettled)
            .into_iter()
            .partition(|(_, heard_ms)| now_ms.saturating_sub(*heard_ms) >= settle_ms);
        self.unsettled = later;
        due.into_iter().map(|(orphaned, _)| orphaned).collect()
    }

    /// How long until a need held now is due to be told, or a helper at rest or a closed maker's
    /// notice has settled. A need waiting for its maker to be free is looked at again when the
    /// maker changes, which wakes the doorbell's thread anyway; and no look is more than a minute
    /// away, nor later than a dialog is due to be told to the person.
    fn due(&self, now_ms: u64) -> Duration {
        let settling = self
            .at_rest_since_ms
            .values()
            .chain(self.unsettled.iter().map(|(_, heard_ms)| heard_ms))
            .chain(self.agent_awaited.then_some(&now_ms))
            .map(|since_ms| {
                Duration::from_millis(
                    since_ms.saturating_add(self.settle_ms).saturating_sub(now_ms),
                )
            });
        self.watches
            .values()
            .filter(|watch| !watch.told && !watch.held)
            .filter_map(|watch| watch.need.map(|(need, since)| (need, since, watch.retry_at_ms)))
            .map(|(need, since_ms, retry_at)| {
                let limit = if need == Need::Waiting { self.waiting_ms } else { self.settle_ms };
                let left = since_ms
                    .saturating_add(limit)
                    .max(retry_at.unwrap_or(0))
                    .saturating_sub(now_ms);
                Duration::from_millis(left)
            })
            .chain(settling)
            // Not while the person is out of reach: a dialog overdue for them then would make every
            // look due at once, and the minute's cap looks again soon enough.
            .chain(self.watches.values().filter(|_| self.human_in_reach).filter_map(|watch| {
                let (need, since_ms) = watch.need?;
                let due_ms = since_ms.saturating_add(self.escalate_ms).saturating_sub(now_ms);
                let due = need == Need::Blocked && watch.escalated_for != Some(since_ms);
                due.then(|| Duration::from_millis(due_ms))
            }))
            .chain(self.held_elsewhere.then_some(ASK_BUSY_AGAIN))
            .min()
            .map_or(NOTHING_DUE, |due| due.min(NOTHING_DUE))
    }
}

/// What the person is told of `helper`, at a dialog for `held_ms` that its maker has not answered.
fn escalation(helper: &Helper, called: &BTreeMap<String, String>, held_ms: u64) -> String {
    let pane = called.get(&helper.pane).unwrap_or(&helper.pane);
    format!(
        "{named}, a helper of {maker}, has been at a dialog for {held}, and its maker has not \
         answered it. Only somebody at its pane can answer it now, so Muster tells you, once; \
         `muster pane show --pane {pane}` brings it forward.",
        named = shown(called, &helper.pane),
        maker = shown(called, &helper.maker),
        held = said_as_time(held_ms),
    )
}

/// What `helper` needs now, `worked_since_ms` being when its last stretch of work began.
fn need(helper: &Helper, worked_since_ms: Option<u64>) -> Option<Need> {
    if helper.stopped.is_some() || helper.activity == Some(Activity::Stuck) {
        return Some(Need::Stuck);
    }
    match helper.activity {
        Some(Activity::Blocked) => Some(Need::Blocked),
        Some(Activity::Waiting) if helper.waiting.is_some() => Some(Need::Waiting),
        Some(Activity::Idle) if helper.helpers_at_work => None,
        Some(Activity::Idle) => {
            let worked = worked_since_ms?;
            let posted = helper.last_post_ms.is_some_and(|posted| posted >= worked);
            (!posted).then_some(Need::Unreported)
        }
        _ => None,
    }
}

/// What the maker is told: the MIP's own sentence, the stall watch's reason, and what the maker
/// can do about it. A block says how long it has held, since a busy maker hears of it late.
fn body(
    helper: &Helper,
    need: Need,
    called: &BTreeMap<String, String>,
    worked_ms: u64,
    held_ms: u64,
    now_ms: u64,
) -> String {
    let named = shown(called, &helper.pane);
    // What a command takes it by: its name, which `--pane` takes as it takes the id.
    let pane = called.get(&helper.pane).unwrap_or(&helper.pane);
    match need {
        Need::Waiting => format!(
            "{named} had been waiting on {reason} for {held} at {at}. \
             `muster pane read --pane {pane} --turn` shows its last turn. Check whether it still needs help. \
             Muster says this once for each declared wait.",
            reason = helper.waiting.as_ref().map_or("its work", |wait| wait.reason.as_str()),
            held = said_as_time(held_ms),
            at =
                muster_core::diagnostics::format_iso8601(i64::try_from(now_ms).unwrap_or(i64::MAX)),
        ),
        Need::Blocked => format!(
            "{named} is blocked at a dialog: {AT_A_DIALOG}, and has been for {held}. `muster pane \
             read --pane {pane}` shows it and `muster pane send --pane {pane} --key <key>` answers it, or bring it \
             forward with `muster pane show --pane {pane}` and ask the person. Muster says this \
             once each time it blocks.",
            held = said_as_time(held_ms),
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
        Need::Unreported => {
            let why = helper.ran_out.as_ref().map_or_else(
                || "its turn may have ended on an error, or it stopped without saying so".to_string(),
                |waited| {
                    let limit = said_as_time(waits::limit().as_millis().try_into().unwrap_or(u64::MAX));
                    format!("the wait it declared on {waited} ran out after {limit} with nothing ending it")
                },
            );
            // A maker that told it to stand by wanted no reply, and learns here how to say so.
            let no_reply = if helper.ran_out.is_none() {
                " If you told it to stand by or that nothing further is needed, post that with \
                 `muster msg post --no-reply` and Muster will not say this."
            } else {
                ""
            };
            format!(
                "{named} went idle without posting since it went to work {worked} ago: {why}. \
                 `muster pane read --pane {pane} --turn` shows what it printed.{no_reply} Muster \
                 says this once each time it goes idle.",
                worked = said_as_time(worked_ms),
            )
        }
    }
}

/// Looks at each helper here, and tells its maker what it needs. Called by the doorbell's thread
/// with no lock held, on every wake; says how long until the next look is due.
pub(crate) fn look(shared: &Arc<Shared>, makers: &mut Makers) -> Duration {
    let (fresh, helping) = {
        let mut session = shared.lock();
        makers.carried.append(&mut session.take_carried_told());
        for helper in session.take_reopens() {
            reopen_with_maker(shared, &mut session, &helper);
        }
        (session.take_orphans(), session.has_helpers())
    };
    let now = now_ms();
    makers.unsettled.extend(fresh.into_iter().map(|orphaned| (orphaned, now)));
    if makers.unsettled.is_empty() && makers.held.is_empty() && !helping {
        makers.watches.clear();
        makers.at_rest_since_ms.clear();
        makers.agent_awaited = false;
        keep_told(shared, makers);
        return NOTHING_DUE;
    }
    let panes = Panes::of(shared);
    makers.carried.retain(|pane, _| panes.exists(pane));
    // Before the notices, so that the person is told of no helper that closes on this look.
    let mut closed = HashSet::new();
    makers.agent_awaited =
        panes.closing().any(|(pane, at_rest)| !at_rest && panes.get(pane).is_none());
    for pane in makers.settled(panes.closing(), now) {
        if shared.lock().close_with_maker(&pane) {
            makers.at_rest_since_ms.remove(&pane);
            closed.insert(pane);
        } else {
            // It went to work or was taken over since it was read: weighed afresh.
            makers.at_rest_since_ms.insert(pane, now);
        }
    }
    let settled = makers.settled_notices(now);
    if !settled.is_empty() || !makers.held.is_empty() {
        let in_reach = shared.messages().service.human_in_reach(&panes);
        for orphaned in settled.iter().filter(|_| !in_reach) {
            log::info(
                "msg.helper.orphans_held",
                fields! { "maker" => orphaned.maker, "helpers" => names(orphaned).join(",") },
            );
        }
        makers.held.extend(settled);
        if in_reach {
            for orphaned in std::mem::take(&mut makers.held) {
                if let Some(open) = still_open(orphaned, &panes, &closed) {
                    tell_person(shared, &panes, &open);
                }
            }
        }
    }
    find_elsewhere(shared, &panes, makers);
    ask_busy_elsewhere(shared, &panes, makers);
    let (helpers, called) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return NOTHING_DUE;
        }
        makers.human_in_reach = messages.service.human_in_reach(&panes);
        let helpers = helpers(&messages.service, &panes, makers, now_ms());
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
    let tells = makers.judge(&helpers, &called, now);
    for (helper, maker, need) in makers.newly_held.drain(..) {
        log::info(
            "msg.helper.held",
            fields! { "helper" => helper, "maker" => maker, "need" => need.as_str() },
        );
    }
    tell_escalations(shared, &panes, makers);
    for tell in tells {
        if tell_maker(shared, &panes, &tell) {
            if let Some(since) = tell.wait_started_ms {
                shared.lock().wait_noticed(&tell.helper, since);
            }
        } else {
            // Whoever is in the maker's pane there may have changed: asked again next time.
            makers.there.remove(&tell.maker);
            if tell.need == Need::Waiting
                && let Some(watch) = makers.watches.get_mut(&tell.helper)
            {
                watch.told = false;
                watch.told_wait_since_ms = None;
                watch.retry_at_ms = Some(now_ms().saturating_add(makers.settle_ms));
            }
        }
    }
    keep_told(shared, makers);
    makers.due(now_ms())
}

/// Gives the session what each helper's maker has been told, when that has changed, for a
/// handoff to carry.
fn keep_told(shared: &Shared, makers: &mut Makers) {
    let telling = makers.telling();
    if makers.told != telling {
        shared.lock().set_makers_told(&telling);
        makers.told = telling;
    }
}

/// Whom this watch tells of what `pane` needs, when it is a helper whose maker it can reach: every
/// participant that is the agent in its maker's pane here, or the one another machine said it is.
pub(crate) fn tells(
    service: &muster_msg::Messaging<super::store::Files>,
    panes: &Panes,
    makers: &Makers,
    pane: &str,
) -> Vec<String> {
    let Some(maker) = panes.maker_of(pane) else { return Vec::new() };
    if panes.exists(maker) {
        return service.in_pane(maker).into_iter().map(str::to_string).collect();
    }
    makers.there.get(maker).cloned().into_iter().collect()
}

/// Asks the machines linked to this one which participant each maker not here is, for each
/// maker not yet found and not asked about in the last [`ASK_AGAIN`]. With no lock held: each
/// ask is a call over a link.
fn find_elsewhere(shared: &Shared, panes: &Panes, makers: &mut Makers) {
    let away: BTreeSet<&String> = panes
        .makers()
        .map(|(_, maker)| maker)
        .filter(|maker| !panes.exists(maker) && !makers.there.contains_key(*maker))
        .collect();
    if away.is_empty() || shared.peers.machines().is_empty() {
        return;
    }
    let now = now_ms();
    let again_ms = u64::try_from(ASK_AGAIN.as_millis()).unwrap_or(u64::MAX);
    for maker in away {
        if makers.asked_ms.get(maker).is_some_and(|asked| now.saturating_sub(*asked) < again_ms) {
            continue;
        }
        match super::whom(shared, maker).unwrap_or_default().as_slice() {
            [there] => {
                makers.asked_ms.remove(maker);
                makers.there.insert(maker.clone(), there.clone());
            }
            _ => {
                makers.asked_ms.insert(maker.clone(), now);
            }
        }
    }
}

/// Asks the machine of each maker on another machine whose helper needs something not yet told
/// whether the maker is at work, in the group Muster would tell it in: a need is told once its
/// maker can hear it, as one whose maker is here is, so a dialog answered meanwhile is never told.
/// The look that takes the answer judges the helper as it is then.
fn ask_busy_elsewhere(shared: &Arc<Shared>, panes: &Panes, makers: &Makers) {
    let asking: Vec<(&String, &String)> = panes
        .makers()
        .filter(|(helper, maker)| {
            !panes.exists(maker)
                && makers
                    .watches
                    .get(*helper)
                    .is_some_and(|watch| watch.need.is_some() && !watch.told)
        })
        .filter_map(|(helper, maker)| Some((helper, makers.there.get(maker)?)))
        .collect();
    if asking.is_empty() {
        return;
    }
    let groups: Vec<Option<String>> = {
        let messages = shared.messages();
        asking.iter().map(|(helper, to)| messages.service.group_told_in(to, helper)).collect()
    };
    for ((_, to), group) in asking.into_iter().zip(groups) {
        makers.busy.ask(shared, to, group);
    }
}

/// Every helper here with an agent in it whose maker is a pane here, or one a linked machine
/// said it has.
fn helpers(
    service: &muster_msg::Messaging<super::store::Files>,
    panes: &Panes,
    makers: &mut Makers,
    now_ms: u64,
) -> Vec<Helper> {
    let mut helpers = Vec::new();
    for (pane, maker) in panes.makers() {
        let to = if panes.exists(maker) {
            maker.clone()
        } else if let Some(there) = makers.there.get(maker) {
            there.clone()
        } else {
            if makers.elsewhere.insert(pane.clone()) {
                log::debug(
                    "msg.helper.maker_elsewhere",
                    fields! {
                        "helper" => pane,
                        "maker" => maker,
                        "impact" => "its maker is not told when it needs somebody until a machine \
                                     linked to this one says it has the maker's pane",
                        "check" => "whether a window is attached to both machines, which is what \
                                    links them",
                    },
                );
            }
            continue;
        };
        let Some(seen) = panes.get(pane) else { continue };
        let idle = seen.activity == Some(Activity::Idle);
        let maker_at_work = panes.get(maker).is_some_and(|maker| {
            matches!(maker.activity, Some(Activity::Working | Activity::Blocked | Activity::Stuck))
        });
        // Asked of whoever is in the maker's pane, which may have joined under a name of its own.
        let hooked =
            service.in_pane(maker).into_iter().any(|name| service.hooked(name, now_ms, panes));
        let maker_here = panes.exists(maker);
        let maker_busy = if maker_here { maker_at_work && !hooked } else { makers.busy.busy(&to) };
        helpers.push(Helper {
            maker_busy,
            maker_here,
            pane: pane.clone(),
            maker: maker.clone(),
            last_post_ms: if idle { service.last_post_by(pane) } else { None },
            helpers_at_work: idle
                && panes.makers().any(|(own, its_maker)| {
                    its_maker == pane
                        && panes.get(own).is_some_and(|own| {
                            matches!(
                                own.activity,
                                Some(Activity::Working | Activity::Waiting | Activity::Blocked)
                            )
                        })
                }),
            asked: matches!(seen.activity, Some(Activity::Working | Activity::Waiting))
                && (seen.io.turn_typed_by() == Some(TypedBy::Someone)
                    || service.asked_since(pane, service.last_post_by(pane).unwrap_or(0))),
            to,
            activity: seen.activity,
            chores_turn: seen.io.turn_typed_by() == Some(TypedBy::Chore),
            turn: seen.io.turn_serial(),
            ran_out: seen.ran_out.clone(),
            waiting: seen.waiting.clone(),
            stopped: seen.stopped.clone(),
        });
    }
    helpers
}

/// Reopens `helper`, which closed with a maker that has just been reopened, as a request to
/// reopen it would: reserved now, under the caller's lock, and started on a thread of its own,
/// since a reopen waits for what the pane ran to end.
fn reopen_with_maker(shared: &Arc<Shared>, session: &mut Session, helper: &str) {
    let reopen = proto::pane_request::Reopen {
        pane: helper.to_string(),
        tab: Minter::new(Mint::Drawn).tab().to_string(),
    };
    let starting = match session.prepare_reopen(reopen, true) {
        Ok(starting) => starting,
        Err(reply) => return not_reopened(helper, &reply.reason),
    };
    let slot = Arc::new(Mutex::new(Some(starting)));
    let spawned = std::thread::Builder::new().name("reopen helper".to_string()).spawn({
        let (shared, slot, helper) = (Arc::clone(shared), Arc::clone(&slot), helper.to_string());
        move || {
            let Some(mut starting) = poison::lock(&slot, "reopen").take() else { return };
            let started = starting.start();
            let reply = shared.lock().started(starting, started);
            if reply.outcome != proto::Outcome::Done {
                not_reopened(&helper, &reply.reason);
            }
        }
    });
    if let Err(error) = spawned
        && let Some(starting) = poison::lock(&slot, "reopen").take()
    {
        let reply = session.started(starting, Err(error));
        not_reopened(helper, &reply.reason);
    }
}

fn not_reopened(helper: &str, reason: &str) {
    log::warn(
        "daemon.helper.not_reopened",
        fields! {
            "pane" => helper,
            "reason" => reason,
            "impact" => "its maker is back without this helper, which closed with it",
            "check" => "`muster pane reopen --list`: while the helper is listed, `muster pane \
                        reopen --pane` with its callsign brings it back",
        },
    );
}

fn names(orphaned: &Orphaned) -> Vec<&str> {
    orphaned.helpers.iter().map(|(_, name)| name.as_str()).collect()
}

/// `orphaned` with only the helpers still open and still to close once idle, or nothing when none
/// is. A helper closes with its maker once it is at rest, which `closed` did since `panes` was
/// read, or after its maker in one request, as a tab holding both does; and one its maker took
/// back, reopened within the settle, or somebody took over is no longer told of.
fn still_open(mut orphaned: Orphaned, panes: &Panes, closed: &HashSet<String>) -> Option<Orphaned> {
    orphaned.helpers.retain(|(pane, _)| panes.closes_once_idle(pane) && !closed.contains(pane));
    if orphaned.helpers.is_empty() {
        log::info("msg.helper.orphans_closed", fields! { "maker" => orphaned.maker });
        return None;
    }
    Some(orphaned)
}

/// What the person is told of a maker that closed with helpers still at work, which close once
/// they are idle unless the person brings them forward.
///
/// The command names the maker by its id, which its helpers still carry: its name went with it.
fn orphaned_body(orphaned: &Orphaned) -> String {
    let (maker, id) = (&orphaned.called, &orphaned.maker);
    match names(orphaned).as_slice() {
        [only] => format!(
            "{maker} closed; its helper {only} is still running, and closes once it is idle. \
             `muster pane show --helpers-of {id}` brings it forward, and it then stays open until \
             you close it."
        ),
        helpers => format!(
            "{maker} closed; {count} of its helpers are still running, and each closes once it is \
             idle: {names}. `muster pane show --helpers-of {id}` brings them forward, and they \
             then stay open until you close them.",
            count = helpers.len(),
            names = helpers.join(", "),
        ),
    }
}

fn tell_person(shared: &Arc<Shared>, panes: &Panes, orphaned: &Orphaned) {
    let body = orphaned_body(orphaned);
    let called = Some(orphaned.called.as_str());
    let untold = {
        let orphaned = orphaned.clone();
        move |why: &str| orphans_untold(&orphaned, why)
    };
    let told = "msg.helper.orphans_told";
    notices::tell_the_person(shared, panes, &orphaned.maker, called, &body, told, untold);
}

fn orphans_untold(orphaned: &Orphaned, reason: &str) {
    log::warn(
        "msg.helper.orphans_untold",
        fields! {
            "maker" => orphaned.maker,
            "helpers" => names(orphaned).join(","),
            "reason" => reason,
            "impact" => "the person was not told that these helpers are still running and \
                         close once idle; they are no longer quiet, so a block notifies as \
                         any pane's does",
            "check" => "the reason: the person homed on another machine, where Muster \
                        cannot tell them from here, or a store that would not write",
        },
    );
}

/// Tells a helper's maker what the helper needs, and says whether it reached them: in the group
/// the two share where it is kept, which may be the maker's machine, or else in one kept here. A
/// notice the group's home is asked to post counts as reached once it is asked, and a refusal
/// from there is logged and not tried again.
fn tell_maker(shared: &Arc<Shared>, panes: &Panes, tell: &Tell) -> bool {
    log::info(
        "msg.helper.needs",
        fields! { "helper" => tell.helper, "maker" => tell.maker, "need" => tell.need.as_str() },
    );
    let routed = shared.messages().service.route_tell_as_muster(
        &tell.to,
        &tell.helper,
        None,
        &tell.body,
        panes,
    );
    match routed {
        Ok(Some(away)) => {
            let (helper, maker, need) = (tell.helper.clone(), tell.maker.clone(), tell.need);
            let told = "msg.helper.told";
            notices::post_at_home(shared, away, told, tell.body.len(), move |why| {
                helper_untold(&helper, &maker, need, why);
            });
            return true;
        }
        Ok(None) => {}
        Err(refusal) => {
            helper_untold(&tell.helper, &tell.maker, tell.need, &words(&refusal));
            return false;
        }
    }
    let delivered = delivering(shared, panes, |service| {
        service.tell_as_muster(&tell.to, &tell.helper, None, &tell.body, panes, now_ms())
    });
    match delivered {
        Ok(delivered) => {
            told("msg.helper.told", Some(tell.body.len()), &delivered);
            !delivered.posted.reached.iter().any(|(_, reach)| *reach == Reach::Unreachable)
        }
        Err(reply) => {
            helper_untold(&tell.helper, &tell.maker, tell.need, &reply.reason);
            false
        }
    }
}

/// Tells the person of each helper whose dialog its maker left, as the last judge found them.
fn tell_escalations(shared: &Arc<Shared>, panes: &Panes, makers: &mut Makers) {
    for (helper, body) in makers.escalations.drain(..) {
        let untold = {
            let helper = helper.clone();
            move |why: &str| escalation_untold(&helper, why)
        };
        let called = panes.name_of(&helper);
        let told = "msg.helper.escalated";
        notices::tell_the_person(shared, panes, &helper, called.as_deref(), &body, told, untold);
    }
}

/// Logs that the person was not told of a helper's dialog its maker left, and why not.
fn escalation_untold(helper: &str, reason: &str) {
    log::warn(
        "msg.helper.unescalated",
        fields! {
            "helper" => helper,
            "reason" => reason,
            "impact" => "a helper has sat at a dialog for half an hour that its maker has not \
                         answered, and the person was not told; nothing answers it",
            "check" => "the reason; the helper's pane, and whether its maker is itself waiting",
        },
    );
}

fn helper_untold(helper: &str, maker: &str, need: Need, reason: &str) {
    log::warn(
        "msg.helper.untold",
        fields! {
            "helper" => helper,
            "maker" => maker,
            "need" => need.as_str(),
            "reason" => reason,
            "impact" => "the helper is quiet, so nobody hears that it needs somebody: its \
                         maker was not told, and the person is not told while it is quiet",
            "check" => "the reason: a maker on another machine whose link dropped, or a \
                        store that would not write; `muster pane quiet --pane <helper> \
                        --off` makes it ask the person instead",
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(since_ms: u64) -> Helper {
        Helper {
            waiting: Some(Waiting { reason: "the full gate".into(), since_ms, noticed: false }),
            ..maple(Activity::Waiting)
        }
    }

    #[test]
    fn a_long_wait_is_told_once_per_declaration_without_ending_it() {
        let mut makers = Makers::after(Duration::from_secs(10));
        let limit = 600_000;
        assert!(makers.judge(&[waiting(100)], &BTreeMap::new(), 100 + limit - 1).is_empty());
        let told = makers.judge(&[waiting(100)], &BTreeMap::new(), 100 + limit);
        assert_eq!(needs(&told), [Need::Waiting]);
        assert!(told[0].body.contains("waiting on the full gate for 10 minutes"));
        assert!(told[0].body.contains("at 1970-01-01T00:10:00.100Z."));
        assert!(!told[0].body.contains("still active"));
        assert!(makers.judge(&[waiting(100)], &BTreeMap::new(), 100 + limit * 2).is_empty());
        assert!(
            makers.judge(&[waiting(100 + limit * 2)], &BTreeMap::new(), 100 + limit * 2).is_empty()
        );
        assert_eq!(
            needs(&makers.judge(&[waiting(100 + limit * 2)], &BTreeMap::new(), 100 + limit * 3)),
            [Need::Waiting]
        );
    }

    #[test]
    fn a_busy_maker_hears_a_long_wait_only_if_it_still_holds() {
        let mut makers = Makers::after(Duration::from_secs(10));
        let busy = Helper { maker_busy: true, ..waiting(100) };
        assert!(makers.judge(&[busy], &BTreeMap::new(), 600_100).is_empty());
        assert_eq!(
            needs(&makers.judge(&[waiting(100)], &BTreeMap::new(), 600_200)),
            [Need::Waiting]
        );

        let mut makers = Makers::after(Duration::from_secs(10));
        let busy = Helper { maker_busy: true, ..waiting(100) };
        makers.judge(&[busy], &BTreeMap::new(), 600_100);
        let finished = Helper { last_post_ms: Some(600_200), ..maple(Activity::Idle) };
        assert!(makers.judge(&[finished], &BTreeMap::new(), 600_200).is_empty());
    }

    #[test]
    fn an_unrelated_turn_and_a_handoff_do_not_repeat_a_wait_notice() {
        let mut makers = Makers::after(Duration::from_secs(10));
        assert_eq!(
            needs(&makers.judge(&[waiting(100)], &BTreeMap::new(), 600_100)),
            [Need::Waiting]
        );
        makers.judge(
            &[Helper { activity: Some(Activity::Working), ..waiting(100) }],
            &BTreeMap::new(),
            600_200,
        );
        assert!(makers.judge(&[waiting(100)], &BTreeMap::new(), 600_300).is_empty());
        let mut next = Makers::after(Duration::from_secs(10));
        let carried = Helper {
            waiting: Some(Waiting { noticed: true, ..waiting(100).waiting.unwrap() }),
            ..maple(Activity::Waiting)
        };
        assert!(next.judge(&[carried], &BTreeMap::new(), 600_400).is_empty());
    }

    const SETTLE_MS: u64 = 10_000;

    fn makers() -> Makers {
        Makers::after(Duration::from_millis(SETTLE_MS))
    }

    /// `maple` in `p2`, made by `p1`, in `activity`.
    fn maple(activity: Activity) -> Helper {
        Helper {
            pane: "p2".to_string(),
            maker: "p1".to_string(),
            to: "p1".to_string(),
            activity: Some(activity),
            chores_turn: false,
            turn: 1,
            ran_out: None,
            waiting: None,
            stopped: None,
            last_post_ms: None,
            helpers_at_work: false,
            asked: true,
            maker_busy: false,
            maker_here: true,
        }
    }

    fn called() -> BTreeMap<String, String> {
        BTreeMap::from([("p2".to_string(), "maple".to_string())])
    }

    /// Half an hour of `SETTLE_MS` settles, when a helper's dialog is left for the person.
    const ESCALATE_MS: u64 = SETTLE_MS * ESCALATE_AFTER_SETTLES;

    /// Judges `helper` at `0` and every settle from `blocked_at` to `until`, and says what the
    /// person was told on the way.
    fn escalated(makers: &mut Makers, helper: &Helper, blocked_at: u64, until: u64) -> Vec<String> {
        let called = called();
        makers.judge(&[maple(Activity::Working)], &called, 0);
        let mut said = Vec::new();
        for at in (blocked_at..=until).step_by(usize::try_from(SETTLE_MS).unwrap()) {
            makers.judge(std::slice::from_ref(helper), &called, at);
            said.extend(makers.escalations.drain(..).map(|(_, body)| body));
        }
        said
    }

    /// A helper at a dialog its maker has left for half an hour is told to the person, once,
    /// whether the maker was told and did nothing or was too busy to be told.
    #[test]
    fn a_dialog_its_maker_left_for_half_an_hour_is_told_to_the_person_once() {
        let blocked = maple(Activity::Blocked);
        let said = escalated(&mut makers(), &blocked, 1_000, 1_000 + ESCALATE_MS + 5 * SETTLE_MS);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with("maple (p2), a helper of p1, has been at a dialog for 30 minutes"),
            "{}",
            said[0]
        );
        assert!(said[0].contains("`muster pane show --pane maple`"), "{}", said[0]);

        let busy = Helper { maker_busy: true, ..maple(Activity::Blocked) };
        let said = escalated(&mut makers(), &busy, 1_000, 1_000 + ESCALATE_MS);
        assert_eq!(said.len(), 1, "a maker too busy to be told");
    }

    /// A dialog answered in time, or a helper that stopped reading, which its maker can restart,
    /// is not the person's to hear of; nor is anything while the person is out of reach, until
    /// they are back.
    #[test]
    fn a_dialog_answered_in_time_or_a_stuck_helper_is_not_told_to_the_person() {
        let mut watch = makers();
        let called = called();
        escalated(&mut watch, &maple(Activity::Blocked), 1_000, ESCALATE_MS);
        watch.judge(&[maple(Activity::Working)], &called, ESCALATE_MS + 1);
        assert!(
            escalated(&mut watch, &maple(Activity::Working), ESCALATE_MS + 2, 2 * ESCALATE_MS)
                .is_empty()
        );

        let stuck = escalated(&mut makers(), &maple(Activity::Stuck), 1_000, 2 * ESCALATE_MS);
        assert!(stuck.is_empty(), "{stuck:?}");

        let mut watch = makers();
        watch.human_in_reach = false;
        let away = escalated(&mut watch, &maple(Activity::Blocked), 1_000, 1_000 + ESCALATE_MS);
        assert!(away.is_empty(), "out of reach");
        let overdue = watch.due(1_000 + ESCALATE_MS + 1);
        assert!(overdue > Duration::ZERO, "looked at again at once, over and over: {overdue:?}");
        watch.human_in_reach = true;
        watch.judge(&[maple(Activity::Blocked)], &called, 1_000 + ESCALATE_MS + SETTLE_MS);
        assert_eq!(watch.escalations.len(), 1, "once back in reach");
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
        assert!(tells[0].body.contains(", and has been for 10s."), "{}", tells[0].body);
        assert!(tells[0].body.contains("--pane maple"), "{}", tells[0].body);
        assert!(makers.judge(&[maple(Activity::Blocked)], &called, 60_000).is_empty(), "once");

        makers.judge(&[maple(Activity::Working)], &called, 61_000);
        makers.judge(&[maple(Activity::Blocked)], &called, 62_000);
        let again = makers.judge(&[maple(Activity::Blocked)], &called, 72_000);
        assert_eq!(needs(&again), [Need::Blocked], "blocked again");
    }

    /// A block that comes due while the maker is at work waits for the maker to be free, is told
    /// then while it still holds, and says how long it has; the thread is not asked to look
    /// again for it meanwhile.
    #[test]
    fn a_block_waits_for_a_busy_maker_and_is_told_once_it_is_free() {
        let mut makers = makers();
        let called = called();
        let busy = |activity| Helper { maker_busy: true, ..maple(activity) };
        makers.judge(&[busy(Activity::Working)], &called, 0);
        makers.judge(&[busy(Activity::Blocked)], &called, 1_000);
        assert!(makers.judge(&[busy(Activity::Blocked)], &called, 30_000).is_empty());
        assert_eq!(makers.due(30_000), NOTHING_DUE);
        assert_eq!(makers.newly_held, [("p2".to_string(), "p1".to_string(), Need::Blocked)]);

        let tells = makers.judge(&[maple(Activity::Blocked)], &called, 40_000);
        assert_eq!(needs(&tells), [Need::Blocked]);
        assert!(tells[0].body.contains("answer it, and has been for 39s."), "{}", tells[0].body);
        assert!(makers.judge(&[maple(Activity::Blocked)], &called, 50_000).is_empty(), "once");
    }

    /// A block answered while its maker was busy, as a maker approving its helpers' plans
    /// answers each, is never told.
    #[test]
    fn a_block_answered_while_the_maker_was_busy_is_not_told() {
        let mut makers = makers();
        let called = called();
        let busy = |activity| Helper { maker_busy: true, ..maple(activity) };
        makers.judge(&[busy(Activity::Working)], &called, 0);
        makers.judge(&[busy(Activity::Blocked)], &called, 1_000);
        assert!(makers.judge(&[busy(Activity::Blocked)], &called, 20_000).is_empty());
        assert!(makers.judge(&[busy(Activity::Working)], &called, 21_000).is_empty());
        assert!(makers.judge(&[maple(Activity::Working)], &called, 90_000).is_empty());
    }

    /// A need held for a maker on another machine has its machine asked again every few seconds,
    /// since nothing there wakes this watch when the maker comes free; one held for a maker here
    /// does not.
    #[test]
    fn a_need_held_for_a_busy_maker_elsewhere_is_looked_at_again_soon() {
        let mut makers = makers();
        let called = called();
        let away = |activity| Helper { maker_busy: true, maker_here: false, ..maple(activity) };
        makers.judge(&[away(Activity::Working)], &called, 0);
        makers.judge(&[away(Activity::Blocked)], &called, 1_000);
        assert!(makers.judge(&[away(Activity::Blocked)], &called, 30_000).is_empty());
        assert_eq!(makers.due(30_000), ASK_BUSY_AGAIN);

        assert!(makers.judge(&[away(Activity::Working)], &called, 31_000).is_empty());
        assert_eq!(makers.due(31_000), NOTHING_DUE, "nothing waits on the maker any more");
    }

    /// A helper first seen at a dialog, its going to work missed by a loaded thread, began its
    /// stretch there: a report posted from the dialog counts, and its idle after is not told.
    #[test]
    fn a_stretch_seen_first_at_a_dialog_counts_a_report_posted_from_it() {
        let mut makers = makers();
        let called = called();
        makers.judge(&[maple(Activity::Idle)], &called, 0);
        makers.judge(&[maple(Activity::Blocked)], &called, 1_000);
        let told = makers.judge(&[maple(Activity::Blocked)], &called, 11_000);
        assert_eq!(needs(&told), [Need::Blocked]);
        makers.judge(&[maple(Activity::Working)], &called, 13_000);
        let reported = Helper { last_post_ms: Some(12_000), ..maple(Activity::Idle) };
        assert!(makers.judge(std::slice::from_ref(&reported), &called, 14_000).is_empty());
        assert!(makers.judge(&[reported], &called, 30_000).is_empty());
    }

    /// A dialog at an idle prompt, a folder's trust or a picker, is no work: the helper going idle
    /// after it is not told as having stopped without posting.
    #[test]
    fn a_dialog_at_an_idle_prompt_is_no_stretch_of_work() {
        let mut makers = makers();
        let called = called();
        makers.judge(&[maple(Activity::Idle)], &called, 0);
        makers.judge(&[maple(Activity::Blocked)], &called, 1_000);
        makers.judge(&[maple(Activity::Idle)], &called, 5_000);
        assert!(makers.judge(&[maple(Activity::Idle)], &called, 15_500).is_empty());
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
        assert!(tells[0].body.contains("went idle without posting since it went to work 5"));

        let mut reporting = makers();
        reporting.judge(&[maple(Activity::Working)], &called, 0);
        let reported = Helper { last_post_ms: Some(50_000), ..maple(Activity::Idle) };
        reporting.judge(std::slice::from_ref(&reported), &called, 60_000);
        assert!(reporting.judge(&[reported], &called, 90_000).is_empty(), "it posted its report");
    }

    /// A helper whose declared wait ran out is told of as that, not as a turn that may have ended
    /// on an error: what happened is known.
    #[test]
    fn a_wait_that_ran_out_is_told_as_that() {
        let called = called();
        let mut makers = makers();
        makers.judge(&[maple(Activity::Working)], &called, 0);
        makers.judge(&[maple(Activity::Waiting)], &called, 60_000);
        let ran_out =
            Helper { ran_out: Some("the full gate".to_string()), ..maple(Activity::Idle) };
        makers.judge(std::slice::from_ref(&ran_out), &called, 660_000);
        let tells = makers.judge(&[ran_out], &called, 670_000);
        assert_eq!(needs(&tells), [Need::Unreported]);
        assert!(
            tells[0].body.contains(
                "the wait it declared on the full gate ran out after 10 minutes with nothing \
                 ending it."
            ),
            "{}",
            tells[0].body
        );
        assert!(!tells[0].body.contains("error"), "{}", tells[0].body);
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

    /// A compaction Muster typed at a helper that had reported and gone idle is not it going to
    /// work, so idle again after it is not told of. The prompt a compaction is followed by is
    /// work, and idle after that without a report is.
    #[test]
    fn a_chores_turn_is_not_going_to_work() {
        let called = called();
        let mut makers = makers();
        let posted = |activity, chores_turn| Helper {
            last_post_ms: Some(5_000),
            chores_turn,
            ..maple(activity)
        };
        makers.judge(&[maple(Activity::Working)], &called, 0);
        makers.judge(&[posted(Activity::Idle, false)], &called, 10_000);
        makers.judge(&[posted(Activity::Working, true)], &called, 20_000);
        makers.judge(&[posted(Activity::Idle, true)], &called, 30_000);
        assert!(makers.judge(&[posted(Activity::Idle, true)], &called, 60_000).is_empty());

        makers.judge(&[posted(Activity::Working, false)], &called, 70_000);
        makers.judge(&[posted(Activity::Idle, false)], &called, 80_000);
        let tells = makers.judge(&[posted(Activity::Idle, false)], &called, 90_000);
        assert_eq!(needs(&tells), [Need::Unreported], "the follow-up was work");
    }

    /// A turn the helper's harness started by itself, compacting while idle, is found to be a chore
    /// only once the harness says so, which can come after a look has seen the turn as work. That
    /// stretch is taken back, so idle after it is not told of; a later turn is work again.
    #[test]
    fn a_turn_found_to_be_a_chore_after_it_was_taken_for_work_is_not_work() {
        let called = called();
        let mut makers = makers();
        let on = |turn, activity, chores_turn| Helper {
            last_post_ms: Some(5_000),
            chores_turn,
            turn,
            ..maple(activity)
        };
        makers.judge(&[maple(Activity::Working)], &called, 0);
        makers.judge(&[on(1, Activity::Idle, false)], &called, 10_000);
        makers.judge(&[on(2, Activity::Working, false)], &called, 20_000);
        makers.judge(&[on(2, Activity::Working, true)], &called, 21_000);
        makers.judge(&[on(2, Activity::Idle, true)], &called, 40_000);
        assert!(makers.judge(&[on(2, Activity::Idle, true)], &called, 70_000).is_empty());

        makers.judge(&[on(3, Activity::Working, false)], &called, 80_000);
        makers.judge(&[on(3, Activity::Idle, false)], &called, 90_000);
        let tells = makers.judge(&[on(3, Activity::Idle, false)], &called, 120_000);
        assert_eq!(needs(&tells), [Need::Unreported], "the next turn was work");
    }

    /// A helper whose maker was told it went idle without posting is not told again for the same
    /// stretch of work after a chore turn, whether the chore was known from the start, as a
    /// compaction Muster typed is, or found after a look took it for work.
    #[test]
    fn a_chore_turn_does_not_tell_the_maker_again_of_a_stretch_it_was_told_of() {
        let called = called();
        for found_late in [false, true] {
            let mut makers = makers();
            let on = |turn, activity, chores_turn| Helper { chores_turn, turn, ..maple(activity) };
            makers.judge(&[on(1, Activity::Working, false)], &called, 0);
            makers.judge(&[on(1, Activity::Idle, false)], &called, 10_000);
            let told = makers.judge(&[on(1, Activity::Idle, false)], &called, 30_000);
            assert_eq!(needs(&told), [Need::Unreported]);

            if found_late {
                makers.judge(&[on(2, Activity::Working, false)], &called, 40_000);
            }
            makers.judge(&[on(2, Activity::Working, true)], &called, 41_000);
            makers.judge(&[on(2, Activity::Idle, true)], &called, 50_000);
            let again = makers.judge(&[on(2, Activity::Idle, true)], &called, 80_000);
            assert!(again.is_empty(), "told again (found late: {found_late}): {again:?}");
        }
    }

    /// A turn nothing new started, as a wake with no new message, a daemon handing the pane over
    /// or Muster's own notice does, is not the helper going to work: idle again after its report
    /// is not told of. A turn something new started is work, and idle after it is.
    #[test]
    fn a_turn_nothing_new_started_is_not_going_to_work() {
        let called = called();
        let mut makers = makers();
        let on = |activity, asked| Helper { last_post_ms: Some(5_000), asked, ..maple(activity) };
        makers.judge(&[maple(Activity::Working)], &called, 0);
        makers.judge(&[on(Activity::Idle, false)], &called, 10_000);
        makers.judge(&[on(Activity::Working, false)], &called, 20_000);
        makers.judge(&[on(Activity::Idle, false)], &called, 30_000);
        let tells = makers.judge(&[on(Activity::Idle, false)], &called, 60_000);
        assert!(tells.is_empty(), "nothing new started it: {tells:?}");

        makers.judge(&[on(Activity::Working, false)], &called, 70_000);
        makers.judge(&[on(Activity::Working, true)], &called, 75_000);
        makers.judge(&[on(Activity::Idle, false)], &called, 80_000);
        let tells = makers.judge(&[on(Activity::Idle, false)], &called, 90_000);
        assert_eq!(needs(&tells), [Need::Unreported], "a message came mid-turn");
    }

    /// A helper whose maker was told it went idle without posting is not told again after a
    /// turn nothing new started: the maker would read a second notice with no cause.
    #[test]
    fn a_turn_nothing_new_started_does_not_tell_the_maker_twice() {
        let called = called();
        let mut makers = makers();
        let on = |activity, asked| Helper { asked, ..maple(activity) };
        makers.judge(&[on(Activity::Working, true)], &called, 0);
        makers.judge(&[on(Activity::Idle, false)], &called, 10_000);
        assert_eq!(
            needs(&makers.judge(&[on(Activity::Idle, false)], &called, 20_000)),
            [Need::Unreported]
        );
        makers.judge(&[on(Activity::Working, false)], &called, 80_000);
        makers.judge(&[on(Activity::Idle, false)], &called, 90_000);
        let again = makers.judge(&[on(Activity::Idle, false)], &called, 120_000);
        assert!(again.is_empty(), "told again: {again:?}");
    }

    /// A helper's first work counts whatever started it: one given its prompt on its command
    /// line had nothing typed and no message.
    #[test]
    fn a_helpers_first_work_is_work_whatever_started_it() {
        let called = called();
        let mut makers = makers();
        let on = |activity| Helper { asked: false, ..maple(activity) };
        makers.judge(&[on(Activity::Working)], &called, 0);
        makers.judge(&[on(Activity::Idle)], &called, 10_000);
        assert_eq!(
            needs(&makers.judge(&[on(Activity::Idle)], &called, 20_000)),
            [Need::Unreported]
        );
    }

    /// The person is told every helper a closed maker left running, and how to bring them back.
    #[test]
    fn a_closed_maker_is_said_with_every_helper_it_left_running() {
        let orphaned = |helpers: &[&str]| Orphaned {
            maker: "p1".to_string(),
            called: "🤖 integrator".to_string(),
            helpers: helpers.iter().map(|helper| (String::new(), (*helper).to_string())).collect(),
        };
        assert_eq!(
            orphaned_body(&orphaned(&["🤖 1", "maple", "cobalt"])),
            "🤖 integrator closed; 3 of its helpers are still running, and each closes once it \
             is idle: 🤖 1, maple, cobalt. `muster pane show --helpers-of p1` brings them forward, \
             and they then stay open until you close them."
        );
        assert!(orphaned_body(&orphaned(&["🤖 1"])).starts_with(
            "🤖 integrator closed; its helper 🤖 1 is still running, and closes once it is idle."
        ));
    }

    #[test]
    fn a_helper_whose_maker_closed_closes_once_at_rest_for_the_settle() {
        let mut makers = makers();
        let (maple, cobalt) = ("maple".to_string(), "cobalt".to_string());
        let seen = |maple_rests, cobalt_rests| vec![(&maple, maple_rests), (&cobalt, cobalt_rests)];

        assert_eq!(makers.settled(seen(true, false).into_iter(), 1_000), Vec::<String>::new());
        assert_eq!(makers.due(1_000), Duration::from_secs(10));
        // Cobalt comes to rest later, and its settle is its own.
        assert_eq!(makers.settled(seen(true, true).into_iter(), 6_000), Vec::<String>::new());
        assert_eq!(makers.settled(seen(true, true).into_iter(), 11_000), vec!["maple"]);
        assert_eq!(makers.settled(seen(false, true).into_iter(), 16_000), vec!["cobalt"]);
    }

    #[test]
    fn going_back_to_work_starts_the_settle_again() {
        let mut makers = makers();
        let maple = "maple".to_string();
        let seen = |rests| std::iter::once((&maple, rests));

        assert!(makers.settled(seen(true), 0).is_empty());
        assert!(makers.settled(seen(false), 5_000).is_empty());
        assert!(makers.settled(seen(true), 9_000).is_empty());
        assert!(makers.settled(seen(true), 12_000).is_empty());
        assert_eq!(makers.settled(seen(true), 19_000), vec!["maple"]);
    }

    #[test]
    fn a_closed_makers_notice_is_held_for_the_settle() {
        let mut makers = makers();
        let orphaned = Orphaned {
            maker: "p1".to_string(),
            called: "pine".to_string(),
            helpers: vec![("p2".to_string(), "maple".to_string())],
        };
        makers.unsettled.push((orphaned.clone(), 1_000));

        assert!(makers.settled_notices(10_999).is_empty());
        assert_eq!(makers.due(10_999), Duration::from_millis(1));
        assert_eq!(makers.settled_notices(11_000), vec![orphaned]);
        assert!(makers.settled_notices(30_000).is_empty());
    }

    /// A block the daemon that handed the panes over told of is not told again, while it lasts;
    /// blocking again afterwards is.
    #[test]
    fn a_block_told_before_a_handoff_is_not_told_again() {
        let mut makers = makers();
        let called = called();
        makers.carried = BTreeMap::from([("p2".to_string(), Need::Blocked)]);
        assert!(makers.judge(&[maple(Activity::Blocked)], &called, SETTLE_MS * 3).is_empty());
        assert_eq!(makers.telling(), BTreeMap::from([("p2".to_string(), Need::Blocked)]));

        makers.judge(&[maple(Activity::Working)], &called, SETTLE_MS * 4);
        assert!(makers.telling().is_empty(), "no longer needed");
        makers.judge(&[maple(Activity::Blocked)], &called, SETTLE_MS * 5);
        let tells = makers.judge(&[maple(Activity::Blocked)], &called, SETTLE_MS * 6);
        assert_eq!(needs(&tells), [Need::Blocked]);
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
