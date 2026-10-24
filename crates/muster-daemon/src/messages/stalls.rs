//! Groups held up on a member: a message waits on an agent in a pane here that is not working,
//! and nothing has moved it for [`STALL_AFTER`]; or on one that has read as working without a
//! break for [`WORKED_AFTER`] times as long.
//!
//! The way a group of agents locks up is a message that never gets read: its member's ring never
//! landed, or landed in a program that has stopped reading, or the member sits at a dialog, and
//! whoever posted it ended their turn to wait for the answer. Each such member's daemon watches
//! its own members, since only it can see their panes, and when one has held a group up long
//! enough it writes the whole picture to the run log (`msg.group.stalled`) and does one thing to
//! free it: wakes the member again, when its last wake never landed, or tells whoever is waiting
//! on it why it has not answered, in a message from Muster (`@muster`) that wakes them as any
//! message does. Once each per stall, and never about Muster's own messages, so nothing here
//! rings anybody in a loop. A stall is one unread message: a member that works a turn and still
//! has not read it is held up on the same one, and whoever waits is not told of it twice.
//!
//! What it does not watch: members waiting on each other with nothing unread. A group whose
//! members are all idle with everything read is also what a finished group looks like, and an
//! expectation of a reply is not something Muster can observe.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_msg::{Activity, Holding, Peer, Presence, Ringable, Settled, Via};

use super::doorbell::Held;
use super::makers::{self, Makers};
use super::presence::Panes;
use super::prompt::{Judged, Unseen};
use super::{Delivered, Messages, activities_of, delivering, now_ms, peer, ring, told, words};
use crate::session::Shared;

/// How long a message may wait on a member that is not working before Muster calls its group
/// held up. Well past the doorbell's own retries - a ring is pressed again for thirty seconds,
/// and an agent woken once more as it goes idle - so that what is left is what those could not
/// move.
pub(crate) const STALL_AFTER: Duration = Duration::from_mins(2);

/// How many times [`STALL_AFTER`] a message may wait on a member reading as working without a
/// break before whoever waits is told: 30 minutes, well past a long turn such as a full test run.
/// A member's state reads working from its screen, and a screen the rules misread holds it there
/// for good - a finished Claude Code turn whose last rows looked like its spinner held an
/// integrator's messages for seven hours - while a turn that long is rare enough to say so.
pub(crate) const WORKED_AFTER: u64 = 15;

/// The protocol minor from which a group's home posts Muster's notice for a member on another
/// machine ([`muster_msg::Call::Muster`]).
const MUSTER_SINCE: u32 = 8;

/// A debug build's [`STALL_AFTER`] in milliseconds, when set, so a test need not wait for it.
const STALL_AFTER_SAID: &str = "MUSTER_DAEMON_STALL_MS";

fn stall_after() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(STALL_AFTER_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    STALL_AFTER
}

/// A member a message waits on, as this daemon sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    pub(crate) holding: Holding,
    /// Its agent's state, or nothing when no agent is in its pane any more.
    pub(crate) activity: Option<Activity>,
    /// Whether its agent can be rung: one whose prompt cannot be read never is, and one still
    /// to come in a new pane, as every agent is while a restarted daemon finds them again, is
    /// not held up yet.
    pub(crate) doorbell: Ringable,
    /// A wake for it is queued, rung and not yet taken, or handed to a command.
    pub(crate) waking: bool,
    /// Of those, a ring typed into its prompt that it has not yet taken.
    pub(crate) rung: bool,
    /// What held the doorbell's last look to ring its pane, if anything did.
    pub(crate) held: Option<Held>,
    /// Its pane's size, columns by rows.
    pub(crate) size: Option<(u16, u16)>,
    /// How long since its program last wrote to its screen, in milliseconds: a wake held for
    /// minutes on a screen nothing has redrawn was judged on one frame all along.
    pub(crate) drawn_ms_ago: Option<u64>,
    /// The pane its tab is zoomed on, when the zoom hides its own.
    pub(crate) zoomed_on: Option<String>,
    /// The program in its pane that has stopped reading, and what it left unread.
    pub(crate) stopped: Option<(String, u32)>,
    /// Whom the maker watch tells when it blocks or stops reading, when its pane is a helper's:
    /// its maker (`super::makers`).
    pub(crate) maker: Vec<String>,
    /// The machine keeping its group, as the link to it knows it, when that is another machine:
    /// what whoever waits is told is posted there, in its names.
    pub(crate) home: Option<Peer>,
}

/// What a look decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    /// A member has held its group up long enough to say so, and what will be done about it.
    Stalled { member: Box<Member>, held_ms: u64, unlock: Unlock },
    /// The member's wake never landed; wake it once more.
    WakeAgain { name: String, group: String },
    /// Tell whoever is waiting on the member why it has not answered.
    Tell { group: String, to: Vec<String>, body: String },
    /// A member that held its group up has moved: read, gone to work, or left.
    Moving { name: String, group: String, held_ms: u64 },
}

/// What a stall is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unlock {
    WakeAgain,
    Tell,
    /// Nobody to tell: every message waiting is from somebody no longer reachable here.
    Nothing,
    /// Whoever waits was told of this message already, before a turn the member worked without
    /// reading it.
    ToldAlready,
}

impl Unlock {
    fn as_str(self) -> &'static str {
        match self {
            Unlock::WakeAgain => "wake_again",
            Unlock::Tell => "tell",
            Unlock::Nothing => "nothing",
            Unlock::ToldAlready => "told_already",
        }
    }
}

/// One member holding one group up.
#[derive(Debug)]
struct Episode {
    /// The oldest message it has not read, which names the episode: reading past it ends this
    /// one, and anything older still unread begins another. Working a turn without reading it
    /// does neither.
    first: u64,
    /// When the member was last free to read it: the message's posting, this daemon's start, or
    /// the last time its agent was seen at work since, whichever is latest.
    since_ms: u64,
    stage: Stage,
    /// Whoever waits has been told of it, which is said once whatever comes after.
    told: bool,
    /// A stall of it is in the log with no move recorded since, which its end is recorded for.
    recorded: bool,
}

impl Episode {
    fn new(first: u64, since_ms: u64) -> Episode {
        Episode { first, since_ms, stage: Stage::Watching, told: false, recorded: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Watching,
    WokenAgain { at_ms: u64 },
    Told,
}

/// Every stall this daemon is watching, between looks.
#[derive(Debug)]
pub(crate) struct Stalls {
    after_ms: u64,
    every: Duration,
    due: Instant,
    episodes: HashMap<(String, String), Episode>,
    worked_after_ms: u64,
    /// When each member was last seen at work, or with its agent still to come, in the host's
    /// milliseconds.
    working: HashMap<String, u64>,
    /// Since when each member seen at work has been at work at every look, with something
    /// waiting on it at each.
    working_since: HashMap<String, u64>,
    /// When this daemon started, in the host's milliseconds. Nothing posted before it has been
    /// waiting on this daemon's watch, so no clock starts earlier: a daemon that restarted, or
    /// took over from another, would otherwise find every unread message stalled at its first
    /// look.
    started_ms: u64,
}

impl Stalls {
    pub(crate) fn new(now: Instant) -> Stalls {
        Stalls::after(stall_after(), now, now_ms())
    }

    fn after(after: Duration, now: Instant, started_ms: u64) -> Stalls {
        let after_ms = u64::try_from(after.as_millis()).unwrap_or(u64::MAX);
        // Often enough that a stall is said within a quarter of its own length of starting.
        let every = (after / 4).clamp(Duration::from_millis(250), Duration::from_secs(30));
        Stalls {
            after_ms,
            worked_after_ms: after_ms.saturating_mul(WORKED_AFTER),
            every,
            due: now,
            episodes: HashMap::new(),
            working: HashMap::new(),
            working_since: HashMap::new(),
            started_ms,
        }
    }

    /// Weighs what holds each group now, `members` being every member a message waits on, and
    /// says what to do; `called` is what each pane and each participant named after one is
    /// called, by id, to say them by. Pure, so every rule here is a unit test.
    pub(crate) fn judge(
        &mut self,
        members: &[Member],
        called: &BTreeMap<String, String>,
        now_ms: u64,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        let mut holding: Vec<(String, String)> = Vec::new();
        // Only members something waits on are judged, so a member that read and went on working
        // is never seen at rest here: its run is forgotten rather than carried into a later turn.
        self.working_since.retain(|name, _| members.iter().any(|m| m.holding.name == *name));
        for member in members {
            let key = (member.holding.name.clone(), member.holding.group.clone());
            let at_work = member.activity == Some(Activity::Working);
            if !at_work {
                self.working_since.remove(&key.0);
            }
            if at_work || member.doorbell == Ringable::AgentToCome {
                self.working.insert(key.0.clone(), now_ms);
                let kept = self.keep_across_work(&key, member, now_ms, &mut acts);
                let worked = at_work && self.worked_too_long(member, called, now_ms, &mut acts);
                if kept || worked {
                    holding.push(key);
                }
                continue;
            }
            holding.push(key.clone());
            let free_since = self.working.get(&key.0).copied().unwrap_or(self.started_ms);
            let since_ms = member.holding.first_at_ms.max(free_since);
            let episode = episode(&mut self.episodes, &key, member, since_ms, now_ms, &mut acts);
            episode.since_ms = episode.since_ms.max(since_ms);
            let held_ms = now_ms.saturating_sub(episode.since_ms);
            if held_ms < self.after_ms {
                continue;
            }
            match episode.stage {
                // Told of already, at work or before a turn it worked without reading: what is
                // left is a wake that never landed.
                Stage::Watching if episode.told && unlock(member) != Unlock::WakeAgain => {
                    let unlock = Unlock::ToldAlready;
                    acts.push(Act::Stalled { member: Box::new(member.clone()), held_ms, unlock });
                    episode.recorded = true;
                    episode.stage = Stage::Told;
                }
                Stage::Watching => {
                    let unlock = unlock(member);
                    acts.push(Act::Stalled { member: Box::new(member.clone()), held_ms, unlock });
                    episode.recorded = true;
                    match unlock {
                        Unlock::WakeAgain => {
                            acts.push(Act::WakeAgain { name: key.0.clone(), group: key.1.clone() });
                            episode.stage = Stage::WokenAgain { at_ms: now_ms };
                        }
                        Unlock::Tell => {
                            acts.push(tell(member, called, now_ms, &why(member, called)));
                            episode.stage = Stage::Told;
                            episode.told = true;
                        }
                        Unlock::Nothing | Unlock::ToldAlready => episode.stage = Stage::Told,
                    }
                }
                // Woken again and still held up: what is left is to tell whoever waits.
                Stage::WokenAgain { at_ms } if now_ms.saturating_sub(at_ms) >= self.after_ms => {
                    if !waiting_on(member).is_empty() && !episode.told {
                        acts.push(tell(member, called, now_ms, &why(member, called)));
                        episode.told = true;
                    }
                    episode.stage = Stage::Told;
                }
                Stage::WokenAgain { .. } | Stage::Told => {}
            }
        }
        let moved: Vec<(String, String)> =
            self.episodes.keys().filter(|key| !holding.contains(key)).cloned().collect();
        for key in moved {
            if let Some(episode) = self.episodes.remove(&key)
                && episode.recorded
            {
                acts.push(moving(&key, &episode, now_ms));
            }
        }
        acts
    }

    /// Keeps the episode of `member`, at work now, across its turn: a member that works and
    /// still has not read what waited on it is the same stall, and whoever waits is not told of
    /// it again. One that has read it meanwhile has ended the stall. Says whether one is kept.
    fn keep_across_work(
        &mut self,
        key: &(String, String),
        member: &Member,
        now_ms: u64,
        acts: &mut Vec<Act>,
    ) -> bool {
        let Some(episode) = self.episodes.get_mut(key) else { return false };
        if episode.first != member.holding.first {
            if episode.recorded {
                acts.push(moving(key, episode, now_ms));
            }
            self.episodes.remove(key);
            return false;
        }
        // Gone to work from a stall said while it was free: that much has moved.
        if episode.stage != Stage::Watching {
            if episode.recorded {
                acts.push(moving(key, episode, now_ms));
                episode.recorded = false;
            }
            episode.stage = Stage::Watching;
        }
        true
    }

    /// Whether `member`, at work now, has been at work at every look for as long as a message
    /// has waited on it past [`WORKED_AFTER`]: whoever waits is told so, once, through `acts`.
    fn worked_too_long(
        &mut self,
        member: &Member,
        called: &BTreeMap<String, String>,
        now_ms: u64,
        acts: &mut Vec<Act>,
    ) -> bool {
        let key = (member.holding.name.clone(), member.holding.group.clone());
        let worked_since = *self.working_since.entry(key.0.clone()).or_insert(now_ms);
        let since_ms = member.holding.first_at_ms.max(worked_since).max(self.started_ms);
        if now_ms.saturating_sub(since_ms) < self.worked_after_ms {
            return false;
        }
        let episode = episode(&mut self.episodes, &key, member, since_ms, now_ms, acts);
        if !episode.told {
            let unlock = if waiting_on(member).is_empty() { Unlock::Nothing } else { Unlock::Tell };
            let held_ms = now_ms.saturating_sub(episode.since_ms);
            acts.push(Act::Stalled { member: Box::new(member.clone()), held_ms, unlock });
            episode.recorded = true;
            if unlock == Unlock::Tell {
                let worked = said_as_time(now_ms.saturating_sub(worked_since));
                let why = format!(
                    "it has read as working for {worked} without a break, so it is rung only once \
                     it stops; an urgent post reaches it at work"
                );
                acts.push(tell(member, called, now_ms, &why));
            }
            episode.told = true;
        }
        true
    }

    /// How long until the next look is due.
    fn left(&self, now: Instant) -> Duration {
        self.due.saturating_duration_since(now)
    }
}

/// The episode of `member` holding its group up, begun now if none is, or begun again when it
/// has read some of what waited, since what is left is a stall of its own.
fn episode<'a>(
    episodes: &'a mut HashMap<(String, String), Episode>,
    key: &(String, String),
    member: &Member,
    since_ms: u64,
    now_ms: u64,
    acts: &mut Vec<Act>,
) -> &'a mut Episode {
    let first = member.holding.first;
    let episode = episodes.entry(key.clone()).or_insert_with(|| Episode::new(first, since_ms));
    if episode.first != first {
        if episode.recorded {
            acts.push(moving(key, episode, now_ms));
        }
        *episode = Episode::new(first, since_ms);
    }
    episode
}

/// How a stall is answered: its member woken once more when its last wake never landed and it
/// could take one, and otherwise whoever waits on it told why.
fn unlock(member: &Member) -> Unlock {
    let ringable = matches!(member.activity, Some(Activity::Idle | Activity::Waiting));
    if !member.holding.woken && !member.waking && member.doorbell == Ringable::Rings && ringable {
        Unlock::WakeAgain
    } else if waiting_on(member).is_empty() {
        Unlock::Nothing
    } else {
        Unlock::Tell
    }
}

/// Whoever posted what waits on the member, the member and Muster aside, and its maker while it
/// is blocked or has stopped reading, which the maker watch tells that maker of already.
fn waiting_on(member: &Member) -> Vec<String> {
    let blocked = matches!(member.activity, Some(Activity::Blocked | Activity::Stuck));
    let maker_told = blocked || member.stopped.is_some();
    member
        .holding
        .authors
        .iter()
        .filter(|author| **author != member.holding.name && *author != muster_msg::MUSTER)
        .filter(|author| !(maker_told && member.maker.contains(author)))
        .cloned()
        .collect()
}

/// What whoever waits on `member` is told, `why` being why it has not read.
fn tell(member: &Member, called: &BTreeMap<String, String>, now_ms: u64, why: &str) -> Act {
    let held = &member.holding;
    let waited = said_as_time(now_ms.saturating_sub(held.first_at_ms));
    // Posted where the group is kept, so in that machine's names.
    let written =
        |name: &str| member.home.as_ref().map_or(name.to_string(), |home| home.outward(name));
    let shown = |id: &str| shown_written(called, id, written);
    // A member named after its pane has already said which pane it is.
    let pane =
        if held.pane == held.name { String::new() } else { format!(", {}", shown(&held.pane)) };
    let body = format!(
        "{name} has not read #{first}, from {authors}, {waited} after it was posted: {why}. \
         Muster says this once; `muster window` shows its pane{pane}.",
        name = shown(&held.name),
        first = held.first,
        authors = held.authors.iter().map(|author| shown(author)).collect::<Vec<_>>().join(", "),
    );
    Act::Tell { group: held.group.clone(), to: waiting_on(member), body }
}

/// `ms` as a person says it: seconds under two minutes, whole minutes from there.
pub(super) fn said_as_time(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 120 { format!("{} minutes", seconds / 60) } else { format!("{seconds}s") }
}

/// A participant or a pane as the CLI shows it: `otter (p29te12q4t)`, the id being what still
/// reaches it once another pane goes by the name.
pub(super) fn shown(called: &BTreeMap<String, String>, id: &str) -> String {
    shown_written(called, id, str::to_string)
}

/// [`shown`], with each name written by `written`: as another machine writes it, say.
fn shown_written(
    called: &BTreeMap<String, String>,
    id: &str,
    written: impl Fn(&str) -> String,
) -> String {
    match called.get(id) {
        Some(name) if name != id => format!("{} ({})", written(name), written(id)),
        _ => written(id),
    }
}

/// Why an agent at a dialog has not moved, as both this watch and a helper's notice to its maker
/// (`super::makers`) say it.
pub(super) const AT_A_DIALOG: &str = "it is at a dialog, waiting for somebody to answer it";

/// Why an agent whose program stopped reading its input has not moved, as both this watch and a
/// helper's notice to its maker say it.
pub(super) fn stopped_reading(program: &str, bytes: u32) -> String {
    format!(
        "its program, {program}, has stopped reading its input and left {bytes} bytes unread, so \
         nothing reaches it until somebody interrupts it"
    )
}

/// Why a member has not read what waits on it, as far as this daemon can see.
fn why(member: &Member, called: &BTreeMap<String, String>) -> String {
    if let Some((program, bytes)) = &member.stopped {
        // Named by the command, since whoever is told is usually an agent that can run it. By
        // its name, which `--pane` takes as it takes the id.
        let pane = called.get(&member.holding.pane).unwrap_or(&member.holding.pane);
        return format!(
            "{}; `muster pane restart --pane {pane}` ends it and resumes its session",
            stopped_reading(program, *bytes)
        );
    }
    match member.activity {
        None => "no agent is in its pane any more".to_string(),
        Some(Activity::Blocked) => AT_A_DIALOG.to_string(),
        _ if member.doorbell != Ringable::Rings => {
            "its agent's prompt cannot be read, so it is never rung; it sees the message when it \
             next reads"
                .to_string()
        }
        _ if member.waking => held(member, called),
        _ if member.holding.woken => {
            "it was rung, and has neither read it nor gone to work".to_string()
        }
        _ => "it was rung again, and still has not read it".to_string(),
    }
}

/// Why a member has not read what waits on it, as `muster msg who` says it at any moment: the
/// stall notice's words, and what that notice never meets, since it tells nobody about a member
/// at work and assumes a member at rest has been woken again.
pub(crate) fn unread_why(member: &Member, called: &BTreeMap<String, String>) -> String {
    let rings = member.stopped.is_none() && member.doorbell == Ringable::Rings;
    match member.activity {
        Some(Activity::Working) if rings && member.waking => {
            "it is at work, so it is rung once it is idle; an urgent post reaches it at work"
                .to_string()
        }
        Some(Activity::Working) if rings => "it was rung, and is at work".to_string(),
        Some(Activity::Idle | Activity::Waiting)
            if rings && !member.waking && !member.holding.woken =>
        {
            "no wake for it is on its way; the next post rings it".to_string()
        }
        _ => why(member, called),
    }
}

/// What holds a wake for a member, by what the doorbell last found in its pane.
fn held(member: &Member, called: &BTreeMap<String, String>) -> String {
    let mut said = match (&member.held, member.size) {
        (Some(Held::Unseen(Unseen::NoPrompt, _)), Some((cols, rows))) => format!(
            "a wake for it is held because its prompt is not on its screen, which is \
             {cols}x{rows} and may be too short to show it"
        ),
        (Some(held), _) => format!("a wake for it is held because {}", held.why()),
        (None, _) if member.rung => {
            "a wake for it is typed into its prompt and not yet taken".to_string()
        }
        (None, _) => "a wake for it is waiting to be rung".to_string(),
    };
    if let Some(zoomed) = &member.zoomed_on {
        let _ = write!(said, "; its tab is zoomed on {}, which hides it", shown(called, zoomed));
    }
    said
}

fn moving(key: &(String, String), episode: &Episode, now_ms: u64) -> Act {
    Act::Moving {
        name: key.0.clone(),
        group: key.1.clone(),
        held_ms: now_ms.saturating_sub(episode.since_ms),
    }
}

/// Looks at what holds each group, when a look is due, and acts on it. Called by the doorbell's
/// thread with no lock held; says how long until the next look.
pub(crate) fn look(shared: &Arc<Shared>, stalls: &mut Stalls, makers: &Makers) -> Duration {
    let now = Instant::now();
    if now < stalls.due {
        return stalls.left(now);
    }
    stalls.due = now + stalls.every;
    let panes = Panes::of(shared);
    let (members, others, called) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return stalls.every;
        }
        let mut members = members(&messages, &panes);
        for member in &mut members {
            member.maker = makers::tells(&messages.service, &panes, makers, &member.holding.pane);
        }
        let others = described(&messages.service, &panes, &members);
        let called = called(&mut messages, &panes, &members);
        let homes: Vec<Option<String>> =
            members.iter().map(|member| messages.service.home_of(&member.holding.group)).collect();
        // The links' own lock is taken with this one let go.
        drop(messages);
        for (member, home) in members.iter_mut().zip(homes) {
            member.home =
                home.and_then(|machine| shared.peers.to(&machine)).map(|link| link.peer.clone());
        }
        (members, others, called)
    };
    let acts = stalls.judge(&members, &called, now_ms());
    for act in acts {
        act_on(shared, &panes, act, &others);
    }
    stalls.left(Instant::now())
}

/// Every member a message waits on, as this daemon sees it now.
pub(crate) fn members(messages: &Messages, panes: &Panes) -> Vec<Member> {
    messages
        .service
        .holding()
        .into_iter()
        .map(|holding| {
            let seen = panes.get(&holding.pane);
            Member {
                activity: seen.and_then(|seen| seen.activity),
                doorbell: panes.doorbell(&holding.pane),
                stopped: seen.and_then(|seen| seen.stopped.clone()),
                waking: messages.wakes_for(&holding.name, &holding.group),
                rung: messages.rung.iter().any(|rung| rung.is_for(&holding.name, &holding.group)),
                held: messages.held.get(&holding.pane).cloned(),
                size: seen.map(|seen| {
                    let grid = seen.io.grid();
                    (grid.cols, grid.rows)
                }),
                drawn_ms_ago: seen
                    .and_then(super::presence::Seen::drawn_at)
                    .map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX)),
                zoomed_on: panes.zoomed_on(&holding.pane).map(str::to_string),
                maker: Vec::new(),
                home: None,
                holding,
            }
        })
        .collect()
}

/// What each pane and each participant named after one is called, by id, to say `members` by.
pub(crate) fn called(
    messages: &mut Messages,
    panes: &Panes,
    members: &[Member],
) -> BTreeMap<String, String> {
    let mut called = messages.service.called(panes);
    for member in members {
        for pane in std::iter::once(&member.holding.pane).chain(&member.zoomed_on) {
            if let Some(name) = panes.name_of(pane) {
                called.insert(pane.clone(), name);
            }
        }
    }
    called
}

/// Every member of each group someone holds up, with what it is doing and how much waits on it,
/// for the record of a stall: the whole group as it stood, not only the member holding it.
fn described(
    service: &muster_msg::Messaging<super::store::Files>,
    panes: &Panes,
    members: &[Member],
) -> HashMap<String, String> {
    let mut described = HashMap::new();
    for summary in service.groups() {
        if !members.iter().any(|member| member.holding.group == summary.name) {
            continue;
        }
        let each: Vec<String> = summary
            .members
            .iter()
            .map(|name| {
                let doing = service
                    .participant(name)
                    .and_then(|participant| panes.activity(participant))
                    .map_or("not in a pane here", activity_word);
                let held = members
                    .iter()
                    .find(|member| {
                        member.holding.name == *name && member.holding.group == summary.name
                    })
                    .map_or(String::new(), |member| {
                        format!(", unread from #{}", member.holding.first)
                    });
                format!("{name}: {doing}{held}")
            })
            .collect();
        described.insert(summary.name, each.join("; "));
    }
    described
}

fn activity_word(activity: Activity) -> &'static str {
    match activity {
        Activity::Working => "working",
        Activity::Blocked => "blocked",
        Activity::Idle => "idle",
        Activity::Waiting => "waiting",
        Activity::Stuck => "stuck",
    }
}

fn act_on(shared: &Arc<Shared>, panes: &Panes, act: Act, others: &HashMap<String, String>) {
    match act {
        Act::Stalled { member, held_ms, unlock } => {
            let held = &member.holding;
            let (program, bytes) = member.stopped.clone().unwrap_or_default();
            let size = member.size.map_or(String::new(), |(cols, rows)| format!("{cols}x{rows}"));
            let judged = match &member.held {
                Some(Held::Unseen(_, judged)) => judged.clone(),
                _ => Judged::default(),
            };
            let drawn = member.drawn_ms_ago.map_or(String::new(), |ms| ms.to_string());
            log::warn(
                "msg.group.stalled",
                fields! {
                    "group" => held.group,
                    "member" => held.name,
                    "pane" => held.pane,
                    "activity" => member.activity.map_or("no agent", activity_word),
                    "first" => held.first,
                    "held_ms" => held_ms,
                    "authors" => held.authors.join(","),
                    "woken" => held.woken,
                    "waking" => member.waking,
                    "held" => member.held.as_ref().map_or("", Held::word),
                    "rule" => judged.rule.unwrap_or_default(),
                    "shape" => judged.shape,
                    "title" => judged.title,
                    "drawn_ms_ago" => drawn,
                    "size" => size,
                    "zoomed_on" => member.zoomed_on.clone().unwrap_or_default(),
                    "rings" => member.doorbell == Ringable::Rings,
                    "program" => program,
                    "unread_bytes" => bytes,
                    "members" => others.get(&held.group).cloned().unwrap_or_default(),
                    "unlock" => unlock.as_str(),
                    "impact" => "a message has waited on this member, which is not working, for \
                                 longer than the doorbell's own retries; whoever posted it may \
                                 have ended their turn to wait for an answer that is not coming",
                    "check" => "the member's pane: a program that stopped reading, a dialog, or \
                                a prompt holding something else; `unlock` says what Muster did",
                },
            );
        }
        Act::WakeAgain { name, group } => {
            let holding = {
                let mut messages = shared.messages();
                let Some(wake) = messages.service.wake_again(&name, &group, panes) else { return };
                let via = match &wake.via {
                    Via::Pane(pane) => pane.clone(),
                    Via::Inbox(inbox) => inbox.socket.clone(),
                    Via::Human => muster_msg::HUMAN.to_string(),
                };
                log::info(
                    "msg.group.woken_again",
                    fields! { "group" => group, "member" => name, "via" => via },
                );
                messages.hold(&[wake], &[], panes)
            };
            ring(shared, holding);
        }
        Act::Tell { group, to, body } => {
            let home = shared.messages().service.home_of(&group);
            if let Some(machine) = home {
                tell_at_home(shared, machine, group, to, body);
                return;
            }
            let delivered = delivering(shared, panes, |service| {
                service.post_as_muster(&group, to.clone(), &body, panes, now_ms())
            });
            match delivered {
                Ok(delivered) => {
                    told("msg.group.told", Some(body.len()), &delivered);
                }
                Err(reply) => untold(&group, &to, &reply.reason),
            }
        }
        Act::Moving { name, group, held_ms } => {
            log::info(
                "msg.group.moving",
                fields! { "group" => group, "member" => name, "held_ms" => held_ms },
            );
        }
    }
}

/// Has `machine`, which keeps `group`, post what whoever waits is told, on a thread of its own:
/// the call may wait seconds on the link, and the doorbell's thread rings everybody else's
/// panes meanwhile.
fn tell_at_home(
    shared: &Arc<Shared>,
    machine: String,
    group: String,
    to: Vec<String>,
    body: String,
) {
    let Some(link) = shared.peers.to(&machine) else {
        untold(&group, &to, &format!("no link to {machine}, where {group} is kept, is up"));
        return;
    };
    if link.speaks.minor < MUSTER_SINCE {
        let speaks = format!("{}.{}", link.speaks.major, link.speaks.minor);
        let why = format!(
            "{machine}'s muster-daemon speaks protocol {speaks}, before a group's home posted \
             Muster's notice for a member elsewhere (1.{MUSTER_SINCE})"
        );
        untold(&group, &to, &why);
        return;
    }
    let away = match shared.messages().service.route_muster(&group, &to, &body) {
        Ok(Some(away)) => away,
        Ok(None) => {
            untold(&group, &to, "the group is kept here now; the next stall is told here");
            return;
        }
        Err(refusal) => {
            untold(&group, &to, &words(&refusal));
            return;
        }
    };
    let unstarted = format!("no thread could be started to ask {machine}");
    let (group_said, to_said) = (group.clone(), to.clone());
    let shared = Arc::clone(shared);
    let started = std::thread::Builder::new().name("stall-tell".to_string()).spawn(move || {
        let (settle, holding) = peer::call_away(&shared, &away);
        let rang = ring(&shared, holding);
        match settle.result {
            Ok(Settled::Posted(posted)) => {
                let activities = activities_of(&shared, &posted, &Panes::of(&shared));
                told("msg.group.told", Some(body.len()), &Delivered { posted, rang, activities });
            }
            Ok(other) => untold(&group, &to, &format!("{machine} answered {other:?}")),
            Err(refusal) => untold(&group, &to, &words(&refusal)),
        }
    });
    if let Err(error) = started {
        untold(&group_said, &to_said, &format!("{unstarted}: {error}"));
    }
}

/// Logs that whoever waits on a member was not told why it has not answered, and why not.
fn untold(group: &str, to: &[String], reason: &str) {
    log::warn(
        "msg.group.untold",
        fields! {
            "group" => group,
            "to" => to.join(","),
            "reason" => reason,
            "impact" => "whoever waits on the member was not told why it has not answered; the \
                         stall is in this log, and nowhere else",
            "check" => "the reason: for a group kept on another machine, whether the link to it \
                        is up and its daemon is as new as this one",
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: u64 = 120_000;

    fn stalls() -> Stalls {
        Stalls::after(Duration::from_millis(AFTER), Instant::now(), 0)
    }

    /// `worker` in `p2`, idle, with #4 from `director` unread since 1000 and its ring given up.
    fn worker() -> Member {
        Member {
            holding: Holding {
                name: "worker".to_string(),
                pane: "p2".to_string(),
                group: "crew".to_string(),
                first: 4,
                first_at_ms: 1000,
                authors: vec!["director".to_string()],
                woken: false,
            },
            activity: Some(Activity::Idle),
            doorbell: Ringable::Rings,
            waking: false,
            rung: false,
            held: None,
            size: Some((89, 9)),
            drawn_ms_ago: None,
            zoomed_on: None,
            stopped: None,
            maker: Vec::new(),
            home: None,
        }
    }

    fn kinds(acts: &[Act]) -> Vec<&'static str> {
        acts.iter()
            .map(|act| match act {
                Act::Stalled { .. } => "stalled",
                Act::WakeAgain { .. } => "wake_again",
                Act::Tell { .. } => "tell",
                Act::Moving { .. } => "moving",
            })
            .collect()
    }

    /// A member whose wake never landed is woken once more when it has held its group up long
    /// enough, then, still held, whoever waits on it is told, once; nothing more is said until
    /// it moves.
    #[test]
    fn a_wake_that_never_landed_is_sent_again_and_then_the_poster_told_once() {
        let mut stalls = stalls();
        assert!(
            stalls.judge(&[worker()], &BTreeMap::new(), 1000 + AFTER - 1).is_empty(),
            "not yet"
        );
        let acts = stalls.judge(&[worker()], &BTreeMap::new(), 1000 + AFTER);
        assert_eq!(kinds(&acts), ["stalled", "wake_again"]);

        let mut woken = worker();
        woken.holding.woken = true;
        assert!(stalls.judge(&[woken.clone()], &BTreeMap::new(), 1000 + 2 * AFTER - 1).is_empty());
        let acts = stalls.judge(&[woken.clone()], &BTreeMap::new(), 1000 + 2 * AFTER);
        assert_eq!(kinds(&acts), ["tell"]);
        let Act::Tell { to, body, .. } = &acts[0] else { unreachable!() };
        assert_eq!(to, &["director"]);
        assert!(body.contains("worker has not read #4") && body.contains("rung"), "{body}");

        assert!(
            stalls.judge(&[woken.clone()], &BTreeMap::new(), 1000 + 10 * AFTER).is_empty(),
            "said twice"
        );
        assert_eq!(kinds(&stalls.judge(&[], &BTreeMap::new(), 1000 + 11 * AFTER)), ["moving"]);
    }

    /// Whoever waits is told about the member by the names agents address each other by, each
    /// with the id that still reaches it once another pane goes by the name.
    #[test]
    fn a_tell_names_members_and_panes_as_the_cli_does() {
        let called: BTreeMap<String, String> =
            [("p2", "otter"), ("p1", "heron")].map(|(id, name)| (id.into(), name.into())).into();
        let in_its_pane = Member {
            holding: Holding {
                name: "p2".to_string(),
                authors: vec!["p1".to_string(), "director".to_string()],
                ..worker().holding
            },
            activity: Some(Activity::Blocked),
            ..worker()
        };
        let acts = Stalls::after(Duration::from_millis(AFTER), Instant::now(), 0).judge(
            &[in_its_pane],
            &called,
            1000 + AFTER,
        );
        let Some(Act::Tell { body, .. }) = acts.last() else { panic!("{acts:?}") };
        assert!(
            body.starts_with("otter (p2) has not read #4, from heron (p1), director"),
            "{body}"
        );
        assert!(body.ends_with("shows its pane."), "{body}");

        let joined = Member { activity: Some(Activity::Blocked), ..worker() };
        let acts = stalls().judge(&[joined], &called, 1000 + AFTER);
        let Some(Act::Tell { body, .. }) = acts.last() else { panic!("{acts:?}") };
        assert!(body.starts_with("worker has not read #4"), "{body}");
        assert!(body.ends_with("shows its pane, otter (p2)."), "{body}");
    }

    /// A program that stopped reading cannot be rung, so whoever waits is told, naming it.
    #[test]
    fn a_member_whose_program_stopped_reading_is_explained_to_whoever_waits() {
        let mut stalls = stalls();
        let stuck = Member {
            activity: Some(Activity::Stuck),
            stopped: Some(("codex".to_string(), 2817)),
            ..worker()
        };
        let acts = stalls.judge(&[stuck], &BTreeMap::new(), 1000 + AFTER);
        assert_eq!(kinds(&acts), ["stalled", "tell"]);
        let Act::Tell { body, .. } = &acts[1] else { unreachable!() };
        assert!(body.contains("codex, has stopped reading its input"), "{body}");
        assert!(body.contains("`muster pane restart --pane p2` ends it"), "{body}");
    }

    /// A helper blocked or stuck on its maker's message leaves that maker to the maker watch,
    /// which has told it already, and tells anybody else waiting; idle, it tells the maker too.
    #[test]
    fn a_helper_blocked_on_its_makers_message_leaves_the_maker_to_the_maker_watch() {
        let helper = |activity, authors: &[&str]| Member {
            holding: Holding {
                authors: authors.iter().map(|author| (*author).to_string()).collect(),
                woken: true,
                ..worker().holding
            },
            activity: Some(activity),
            maker: vec!["director".to_string()],
            ..worker()
        };
        let at = 1000 + AFTER;
        for activity in [Activity::Blocked, Activity::Stuck] {
            let acts = stalls().judge(&[helper(activity, &["director"])], &BTreeMap::new(), at);
            assert_eq!(kinds(&acts), ["stalled"], "{activity:?}");
            let Act::Stalled { unlock, .. } = acts[0] else { unreachable!() };
            assert_eq!(unlock, Unlock::Nothing);
        }

        let both = helper(Activity::Blocked, &["director", "critic"]);
        let acts = stalls().judge(&[both], &BTreeMap::new(), at);
        let Some(Act::Tell { to, .. }) = acts.last() else { panic!("{acts:?}") };
        assert_eq!(to, &["critic"]);

        let acts = stalls().judge(&[helper(Activity::Idle, &["director"])], &BTreeMap::new(), at);
        let Some(Act::Tell { to, .. }) = acts.last() else { panic!("{acts:?}") };
        assert_eq!(to, &["director"]);
    }

    /// A member at work is not holding anything up, and the clock starts over from when it was
    /// last seen working: a long turn is not a stall.
    #[test]
    fn work_restarts_the_clock() {
        let mut stalls = stalls();
        let working = Member { activity: Some(Activity::Working), ..worker() };
        assert!(stalls.judge(&[working], &BTreeMap::new(), 1000 + AFTER).is_empty());
        assert!(
            stalls.judge(&[worker()], &BTreeMap::new(), 1000 + 2 * AFTER - 1).is_empty(),
            "free only since"
        );
        assert_eq!(
            kinds(&stalls.judge(&[worker()], &BTreeMap::new(), 1000 + 2 * AFTER)),
            ["stalled", "wake_again"]
        );
    }

    /// Reading some of what waited ends the stall, and what is left is a new one with a clock of
    /// its own.
    #[test]
    fn reading_part_of_it_starts_a_new_stall() {
        let mut stalls = stalls();
        stalls.judge(&[worker()], &BTreeMap::new(), 1000 + AFTER);
        let mut later = worker();
        later.holding.first = 7;
        later.holding.first_at_ms = 1000 + AFTER;
        assert_eq!(
            kinds(&stalls.judge(&[later.clone()], &BTreeMap::new(), 1000 + AFTER + 1)),
            ["moving"]
        );
        assert_eq!(
            kinds(&stalls.judge(&[later], &BTreeMap::new(), 1000 + 2 * AFTER)),
            ["stalled", "wake_again"]
        );
    }

    /// A member told of that works a turn and is free again with the same message unread is the
    /// same stall: it is recorded again, and whoever waits is not told twice.
    #[test]
    fn a_member_that_works_a_turn_without_reading_is_not_told_of_twice() {
        let mut stalls = stalls();
        let none = BTreeMap::new();
        let held = [Member { waking: true, ..worker() }];
        assert_eq!(kinds(&stalls.judge(&held, &none, 1000 + AFTER)), ["stalled", "tell"]);

        let working = [Member { activity: Some(Activity::Working), ..held[0].clone() }];
        let at_work = 1000 + 2 * AFTER;
        assert_eq!(kinds(&stalls.judge(&working, &none, at_work)), ["moving"]);
        assert!(stalls.judge(&working, &none, at_work + 1).is_empty(), "moved twice");

        assert!(stalls.judge(&held, &none, at_work + AFTER - 1).is_empty());
        let acts = stalls.judge(&held, &none, at_work + 1 + AFTER);
        assert_eq!(kinds(&acts), ["stalled"], "told twice");
        assert!(matches!(acts[0], Act::Stalled { unlock: Unlock::ToldAlready, .. }), "{acts:?}");
        assert!(stalls.judge(&held, &none, at_work + 10 * AFTER).is_empty());

        assert_eq!(kinds(&stalls.judge(&[], &none, at_work + 11 * AFTER)), ["moving"]);
    }

    /// A member that reads what it was told of while at work has ended that stall, and what is
    /// left unread is a stall of its own, told of in turn.
    #[test]
    fn reading_at_work_ends_the_stall() {
        let mut stalls = stalls();
        let none = BTreeMap::new();
        let held = Member { waking: true, ..worker() };
        let working = Member { activity: Some(Activity::Working), ..held.clone() };
        stalls.judge(std::slice::from_ref(&held), &none, 1000 + AFTER);
        stalls.judge(std::slice::from_ref(&working), &none, 1000 + 2 * AFTER);

        let later = Holding { first: 7, first_at_ms: 1000 + 2 * AFTER, ..held.holding.clone() };
        let read = Member { holding: later.clone(), ..working };
        assert!(stalls.judge(&[read], &none, 1000 + 3 * AFTER).is_empty());
        let free = Member { holding: later, ..held };
        let acts = stalls.judge(&[free], &none, 1000 + 4 * AFTER);
        assert_eq!(kinds(&acts), ["stalled", "tell"]);
        let Act::Tell { body, .. } = &acts[1] else { unreachable!() };
        assert!(body.contains("has not read #7"), "{body}");
    }

    /// A daemon that has just started, after a restart or taking over from another, holds no
    /// member to account for what waited before it: the clock starts at its start, and a member
    /// whose agent is still to come in its pane is not held up at all until it has come.
    #[test]
    fn a_restarted_daemon_starts_every_clock_at_its_own_start() {
        let started = 1000 + 10 * AFTER;
        let mut stalls = Stalls::after(Duration::from_millis(AFTER), Instant::now(), started);
        let coming = Member { activity: None, doorbell: Ringable::AgentToCome, ..worker() };
        assert!(
            stalls.judge(&[coming], &BTreeMap::new(), started + 1).is_empty(),
            "its agent is resuming"
        );
        assert!(
            stalls.judge(&[worker()], &BTreeMap::new(), started + AFTER).is_empty(),
            "free only since it came"
        );

        let mut stalls = Stalls::after(Duration::from_millis(AFTER), Instant::now(), started);
        assert!(
            stalls.judge(&[worker()], &BTreeMap::new(), started + 1).is_empty(),
            "posted before the start"
        );
        assert!(stalls.judge(&[worker()], &BTreeMap::new(), started + AFTER - 1).is_empty());
        assert_eq!(
            kinds(&stalls.judge(&[worker()], &BTreeMap::new(), started + AFTER)),
            ["stalled", "wake_again"]
        );
    }

    /// A member only Muster has written to holds nothing, and a member nobody else waits on is
    /// recorded but tells nobody.
    #[test]
    fn nobody_is_told_about_what_nobody_waits_for() {
        let mut stalls = stalls();
        let blocked = Member {
            activity: Some(Activity::Blocked),
            holding: Holding { authors: vec![muster_msg::MUSTER.to_string()], ..worker().holding },
            ..worker()
        };
        let acts = stalls.judge(&[blocked], &BTreeMap::new(), 1000 + AFTER);
        assert_eq!(kinds(&acts), ["stalled"]);
        assert!(matches!(acts[0], Act::Stalled { unlock: Unlock::Nothing, .. }));
    }

    /// The body of what whoever waits on `member` is told once it has held its group up.
    fn told(member: Member, called: &BTreeMap<String, String>) -> String {
        let acts = stalls().judge(&[member], called, 1000 + AFTER);
        let Some(Act::Tell { body, .. }) = acts.last() else { panic!("{acts:?}") };
        body.clone()
    }

    /// A held wake is explained by what the doorbell found in the pane, and never by a guess.
    #[test]
    fn a_held_wake_is_told_by_what_held_it() {
        let waking = |held| Member { waking: true, held, ..worker() };
        let none = BTreeMap::new();
        let said = |held| told(waking(held), &none);
        assert!(said(Some(Held::Draft)).contains("its prompt holds something typed and not sent"));
        assert!(
            said(Some(Held::Unseen(Unseen::NoPrompt, Judged::default())))
                .contains("its prompt is not on its screen, which is 89x9 and may be too short"),
        );
        assert!(
            said(Some(Held::Unseen(Unseen::Covered(None), Judged::default())))
                .contains("a menu, a viewer or a dialog")
        );
        let drafted = "Claude Code's box for a bug report it drafted (1 to review, 2 to send, 0 to \
                       dismiss)";
        assert!(
            said(Some(Held::Unseen(Unseen::Covered(Some(drafted.to_string())), Judged::default())))
                .contains(&format!("a wake for it is held because {drafted} covers its prompt")),
        );
        assert!(
            said(Some(Held::Unseen(Unseen::AtWork, Judged::default())))
                .contains("reads as its agent at work")
        );
        assert!(said(None).contains("a wake for it is waiting to be rung."));
        for held in
            [None, Some(Held::Draft), Some(Held::Unseen(Unseen::NoPrompt, Judged::default()))]
        {
            assert!(!said(held.clone()).contains("menu over it"), "{held:?}");
        }
    }

    /// What `who` says of a member at any moment: the held wake in the doorbell's words, as the
    /// stall notice says it, and the cases that notice never meets - a member at work, and one
    /// at rest with no wake on its way.
    #[test]
    fn why_a_member_has_not_read_is_said_whenever_asked() {
        let none = BTreeMap::new();
        let why = |member: Member| unread_why(&member, &none);
        let at_work = Member { activity: Some(Activity::Working), ..worker() };
        assert!(
            why(Member { waking: true, ..at_work.clone() })
                .starts_with("it is at work, so it is rung once it is idle")
        );
        let rung = Holding { woken: true, ..worker().holding };
        assert_eq!(
            why(Member { holding: rung.clone(), ..at_work.clone() }),
            "it was rung, and is at work"
        );
        assert_eq!(why(worker()), "no wake for it is on its way; the next post rings it");
        assert_eq!(
            why(Member { waking: true, rung: true, ..worker() }),
            "a wake for it is typed into its prompt and not yet taken"
        );
        assert_eq!(
            why(Member { waking: true, held: Some(Held::Draft), ..worker() }),
            "a wake for it is held because its prompt holds something typed and not sent"
        );
        assert_eq!(
            why(Member { holding: rung, ..worker() }),
            "it was rung, and has neither read it nor gone to work"
        );
        let blocked = Member { activity: Some(Activity::Blocked), waking: true, ..worker() };
        assert_eq!(why(blocked), "it is at a dialog, waiting for somebody to answer it");
        let stopped = Member { stopped: Some(("codex".to_string(), 12)), ..at_work };
        assert!(why(stopped).starts_with("its program, codex, has stopped reading"));
    }

    /// A pane a zoom hides is said to be hidden, by the name of the pane its tab is zoomed on.
    #[test]
    fn a_held_wake_in_a_pane_a_zoom_hides_says_so() {
        let called: BTreeMap<String, String> = [("p3".to_string(), "heron".to_string())].into();
        let hidden = Member {
            waking: true,
            held: Some(Held::Unseen(Unseen::NoPrompt, Judged::default())),
            zoomed_on: Some("p3".to_string()),
            ..worker()
        };
        let body = told(hidden, &called);
        assert!(
            body.contains(
                "89x9 and may be too short to show it; its tab is zoomed on heron (p3), \
                           which hides it."
            ),
            "{body}"
        );
    }

    /// A message waiting on a member that has read as working at every look is said once it has
    /// waited fifteen times as long as a stall: a screen misread as working holds it there for
    /// good, and nothing else would tell whoever waits.
    #[test]
    fn a_member_working_without_a_break_is_told_of_once() {
        let mut stalls = stalls();
        let working = [Member { activity: Some(Activity::Working), ..worker() }];
        let worked = 1000 + WORKED_AFTER * AFTER;
        for at in [1000, 1000 + AFTER, worked - 1] {
            assert!(stalls.judge(&working, &BTreeMap::new(), at).is_empty(), "at {at}");
        }
        let acts = stalls.judge(&working, &BTreeMap::new(), worked);
        assert_eq!(kinds(&acts), ["stalled", "tell"]);
        let Act::Tell { body, .. } = &acts[1] else { unreachable!() };
        assert!(body.contains("it has read as working for 30 minutes without a break"), "{body}");
        assert!(body.contains("an urgent post reaches it at work"), "{body}");
        assert!(
            stalls.judge(&working, &BTreeMap::new(), worked + AFTER).is_empty(),
            "said twice at work"
        );

        // Free at last, it is not told of again for the same message, though a wake that never
        // landed is still sent again.
        assert!(stalls.judge(&[worker()], &BTreeMap::new(), worked + AFTER + 1).is_empty());
        assert_eq!(
            kinds(&stalls.judge(&[worker()], &BTreeMap::new(), worked + 2 * AFTER)),
            ["stalled", "wake_again"]
        );
        let woken = Member { holding: Holding { woken: true, ..worker().holding }, ..worker() };
        assert!(
            stalls.judge(&[woken], &BTreeMap::new(), worked + 10 * AFTER).is_empty(),
            "told again once free"
        );
    }

    /// A look that finds the member not at work starts its run of work over.
    #[test]
    fn a_break_in_work_starts_the_long_clock_over() {
        let mut stalls = stalls();
        let working = [Member { activity: Some(Activity::Working), ..worker() }];
        let worked = 1000 + WORKED_AFTER * AFTER;
        stalls.judge(&working, &BTreeMap::new(), 1000);
        stalls.judge(&[worker()], &BTreeMap::new(), 1000 + AFTER / 2);
        stalls.judge(&working, &BTreeMap::new(), 1000 + AFTER);
        assert!(stalls.judge(&working, &BTreeMap::new(), worked).is_empty());
        assert_eq!(
            kinds(&stalls.judge(&working, &BTreeMap::new(), worked + AFTER)),
            ["stalled", "tell"]
        );
    }

    /// A member that read what waited and went on working is no longer judged, so no look finds
    /// it at rest. Its run of work is still over: the next message to wait on it through a long
    /// turn says how long that turn has run, not how long since the member was first seen at work.
    #[test]
    fn a_run_of_work_ends_once_nothing_waits_on_the_member() {
        let mut stalls = stalls();
        let working = Member { activity: Some(Activity::Working), ..worker() };
        stalls.judge(std::slice::from_ref(&working), &BTreeMap::new(), 1000);
        stalls.judge(&[], &BTreeMap::new(), 1000 + AFTER);

        let posted = 1000 + 700 * AFTER;
        let later = Member {
            holding: Holding { first: 9, first_at_ms: posted, ..worker().holding },
            ..working
        };
        let worked = posted + WORKED_AFTER * AFTER;
        stalls.judge(std::slice::from_ref(&later), &BTreeMap::new(), posted);
        let acts = stalls.judge(&[later], &BTreeMap::new(), worked);
        let Some(Act::Tell { body, .. }) = acts.last() else { panic!("{acts:?}") };
        assert!(body.contains("it has read as working for 30 minutes without a break"), "{body}");
    }
}
