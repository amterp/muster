//! Groups held up on a member: a message waits on an agent in a pane here that is not working,
//! and nothing has moved it for [`STALL_AFTER`].
//!
//! The way a group of agents locks up is a message that never gets read: its member's ring never
//! landed, or landed in a program that has stopped reading, or the member sits at a dialog, and
//! whoever posted it ended their turn to wait for the answer. Each such member's daemon watches
//! its own members, since only it can see their panes, and when one has held a group up long
//! enough it writes the whole picture to the run log (`msg.group.stalled`) and does one thing to
//! free it: wakes the member again, when its last wake never landed, or tells whoever is waiting
//! on it why it has not answered, in a message from Muster (`@muster`) that wakes them as any
//! message does. Once each per stall, and never about Muster's own messages, so nothing here
//! rings anybody in a loop.
//!
//! What it does not watch: members waiting on each other with nothing unread. A group whose
//! members are all idle with everything read is also what a finished group looks like, and an
//! expectation of a reply is not something Muster can observe.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_msg::{Activity, Holding, Presence, Ringable, Via};

use super::presence::Panes;
use super::{delivering, now_ms, ring, told};
use crate::session::Shared;

/// How long a message may wait on a member that is not working before Muster calls its group
/// held up. Well past the doorbell's own retries - a ring is pressed again for thirty seconds,
/// and an agent woken once more as it goes idle - so that what is left is what those could not
/// move.
pub(crate) const STALL_AFTER: Duration = Duration::from_mins(2);

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
    /// The program in its pane that has stopped reading, and what it left unread.
    pub(crate) stopped: Option<(String, u32)>,
}

/// What a look decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    /// A member has held its group up long enough to say so, and what will be done about it.
    Stalled { member: Member, held_ms: u64, unlock: Unlock },
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
}

impl Unlock {
    fn as_str(self) -> &'static str {
        match self {
            Unlock::WakeAgain => "wake_again",
            Unlock::Tell => "tell",
            Unlock::Nothing => "nothing",
        }
    }
}

/// One member holding one group up.
#[derive(Debug)]
struct Episode {
    /// The oldest message it has not read, which names the episode: reading past it ends this
    /// one, and anything older still unread begins another.
    first: u64,
    /// When the member was last free to read it: the message's posting, this daemon's start, or
    /// the last time its agent was seen at work since, whichever is latest.
    since_ms: u64,
    stage: Stage,
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
    /// When each member was last seen at work, or with its agent still to come, in the host's
    /// milliseconds.
    working: HashMap<String, u64>,
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
            every,
            due: now,
            episodes: HashMap::new(),
            working: HashMap::new(),
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
        for member in members {
            let key = (member.holding.name.clone(), member.holding.group.clone());
            if member.activity == Some(Activity::Working)
                || member.doorbell == Ringable::AgentToCome
            {
                self.working.insert(key.0.clone(), now_ms);
                continue;
            }
            holding.push(key.clone());
            let free_since = self.working.get(&key.0).copied().unwrap_or(self.started_ms);
            let since_ms = member.holding.first_at_ms.max(free_since);
            let episode = self.episodes.entry(key.clone()).or_insert(Episode {
                first: member.holding.first,
                since_ms,
                stage: Stage::Watching,
            });
            if episode.first != member.holding.first {
                // It read some of what waited, and what is left is a stall of its own.
                if episode.stage != Stage::Watching {
                    acts.push(moving(&key, episode, now_ms));
                }
                *episode =
                    Episode { first: member.holding.first, since_ms, stage: Stage::Watching };
            }
            episode.since_ms = episode.since_ms.max(since_ms);
            let held_ms = now_ms.saturating_sub(episode.since_ms);
            if held_ms < self.after_ms {
                continue;
            }
            match episode.stage {
                Stage::Watching => {
                    let unlock = unlock(member);
                    acts.push(Act::Stalled { member: member.clone(), held_ms, unlock });
                    match unlock {
                        Unlock::WakeAgain => {
                            acts.push(Act::WakeAgain { name: key.0.clone(), group: key.1.clone() });
                            episode.stage = Stage::WokenAgain { at_ms: now_ms };
                        }
                        Unlock::Tell => {
                            acts.push(tell(member, called, now_ms));
                            episode.stage = Stage::Told;
                        }
                        Unlock::Nothing => episode.stage = Stage::Told,
                    }
                }
                // Woken again and still held up: what is left is to tell whoever waits.
                Stage::WokenAgain { at_ms } if now_ms.saturating_sub(at_ms) >= self.after_ms => {
                    if !waiting_on(member).is_empty() {
                        acts.push(tell(member, called, now_ms));
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
                && episode.stage != Stage::Watching
            {
                acts.push(moving(&key, &episode, now_ms));
            }
        }
        acts
    }

    /// How long until the next look is due.
    fn left(&self, now: Instant) -> Duration {
        self.due.saturating_duration_since(now)
    }
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

/// Whoever posted what waits on the member, the member and Muster aside.
fn waiting_on(member: &Member) -> Vec<String> {
    member
        .holding
        .authors
        .iter()
        .filter(|author| **author != member.holding.name && *author != muster_msg::MUSTER)
        .cloned()
        .collect()
}

fn tell(member: &Member, called: &BTreeMap<String, String>, now_ms: u64) -> Act {
    let held = &member.holding;
    let waited = now_ms.saturating_sub(held.first_at_ms) / 1000;
    let waited =
        if waited >= 120 { format!("{} minutes", waited / 60) } else { format!("{waited}s") };
    // As the CLI shows a name: `otter (p29te12q4t)`, the id being what still reaches it once
    // another pane goes by the name.
    let shown = |id: &str| match called.get(id) {
        Some(name) if name != id => format!("{name} ({id})"),
        _ => id.to_string(),
    };
    // A member named after its pane has already said which pane it is.
    let pane =
        if held.pane == held.name { String::new() } else { format!(", {}", shown(&held.pane)) };
    let body = format!(
        "{name} has not read #{first}, from {authors}, {waited} after it was posted: {why}. \
         Muster says this once; `muster window` shows its pane{pane}.",
        name = shown(&held.name),
        first = held.first,
        authors = held.authors.iter().map(|author| shown(author)).collect::<Vec<_>>().join(", "),
        why = why(member),
    );
    Act::Tell { group: held.group.clone(), to: waiting_on(member), body }
}

/// Why a member has not read what waits on it, as far as this daemon can see.
fn why(member: &Member) -> String {
    if let Some((program, bytes)) = &member.stopped {
        return format!(
            "its program, {program}, has stopped reading its input and left {bytes} bytes unread, \
             so nothing reaches it until somebody interrupts it, or ends it and resumes its \
             session"
        );
    }
    match member.activity {
        None => "no agent is in its pane any more".to_string(),
        Some(Activity::Blocked) => {
            "it is at a dialog, waiting for somebody to answer it".to_string()
        }
        _ if member.doorbell != Ringable::Rings => {
            "its agent's prompt cannot be read, so it is never rung; it sees the message when it \
             next reads"
                .to_string()
        }
        _ if member.waking => {
            "a wake for it is waiting to be rung, held back by what its pane shows - something \
             typed in its prompt, or a menu over it"
                .to_string()
        }
        _ if member.holding.woken => {
            "it was rung, and has neither read it nor gone to work".to_string()
        }
        _ => "it was rung again, and still has not read it".to_string(),
    }
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
pub(crate) fn look(shared: &Arc<Shared>, stalls: &mut Stalls) -> Duration {
    let now = Instant::now();
    if now < stalls.due {
        return stalls.left(now);
    }
    stalls.due = now + stalls.every;
    let panes = Panes::of(shared);
    let (members, others, mut called) = {
        let mut messages = shared.messages();
        if messages.handing_over {
            return stalls.every;
        }
        let members: Vec<Member> = messages
            .service
            .holding()
            .into_iter()
            .map(|holding| {
                let seen = panes.get(&holding.pane);
                let waking = messages.wakes_for(&holding.name, &holding.group);
                Member {
                    activity: seen.and_then(|seen| seen.activity),
                    doorbell: panes.doorbell(&holding.pane),
                    stopped: seen.and_then(|seen| seen.stopped.clone()),
                    waking,
                    holding,
                }
            })
            .collect();
        let others = described(&messages.service, &panes, &members);
        (members, others, messages.service.called(&panes))
    };
    for member in &members {
        if let Some(name) = panes.name_of(&member.holding.pane) {
            called.insert(member.holding.pane.clone(), name);
        }
    }
    let acts = stalls.judge(&members, &called, now_ms());
    for act in acts {
        act_on(shared, &panes, act, &others);
    }
    stalls.left(Instant::now())
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
            let delivered = delivering(shared, panes, |service| {
                service.post_as_muster(&group, to.clone(), &body, panes, now_ms())
            });
            match delivered {
                Ok(delivered) => {
                    told("msg.group.told", Some(body.len()), &delivered);
                }
                Err(reply) => log::warn(
                    "msg.group.untold",
                    fields! {
                        "group" => group,
                        "to" => to.join(","),
                        "reason" => reply.reason,
                        "impact" => "whoever waits on the member was not told why it has not \
                                     answered; the stall is in this log, and nowhere else",
                        "check" => "whether the group is kept on another machine, where Muster \
                                    cannot post to it from here",
                    },
                ),
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
            stopped: None,
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
}
