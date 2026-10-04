//! The doorbell (MIP-4, section 6): a wake typed into the pane an agent runs in, as one line
//! and a Return, for an agent nothing else can wake - which, since a session that bypasses
//! permission prompts holds what its inbox is sent, is every agent in a pane.
//!
//! What it types is read as an answer by whatever the screen shows, so it types only into what
//! it has seen: the agent at its own prompt, and the prompt empty ([`super::prompt`]). A screen
//! detection does not recognize, a menu, a dialog opened while the agent was idle, a draft half
//! typed - none of these is an empty prompt, so none is rung. Before it looks it waits for the
//! agent to be idle or waiting and for nothing to have been typed into the pane for a few
//! seconds, since a keystroke may not have reached the screen yet. What cannot be rung at once
//! waits here, and this thread rings it when the pane allows. The same thread wakes an agent
//! once more when it goes idle with what it was woken for still unread (section 5).
//!
//! A wake for messages any of which was posted urgently may also ring an agent at work, at a
//! prompt read as empty while it works ([`super::prompt`]): Claude Code queues what is typed
//! there and takes it into the turn it is running. Everything else above holds for it. An agent
//! at work can open a dialog at any moment, after the prompt was read and before what was typed
//! reaches it, and a Return there answers the dialog: Claude Code's permission dialog takes it
//! as its highlighted option, "Yes", while it ignores the line itself, pasted or typed
//! (docs/observations/claude-code-2.1.288.md). So a ring at work is typed without its Return,
//! and the Return follows only once a second look finds the prompt holding the ring and nothing
//! else. A dialog covering it is waited out however long it stays open, since the ring sits in
//! the prompt meanwhile and nothing else would send it. What is left is a dialog drawn between
//! that look and the Return, which this daemon's copy of the screen shows a moment late.
//!
//! Claude Code keeps what is typed while it starts as its prompt and drops the Return, so a
//! ring can sit unsent. Return is pressed again for it, a few times, but only while nobody has
//! typed into the pane since and the prompt holds the ring's own text and nothing else: that
//! Return can send nothing but the ring. The first is what the daemon saw written, since the
//! screen can lag it. A ring given up with its text still in the prompt is sent by the next ring
//! that finds exactly that text there, so a prompt the doorbell filled never reads as somebody's
//! draft for good.

use std::collections::HashMap;
use std::sync::{OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::messaging;
use muster_msg::{Activity, Presence, Ringable, Via, Wake};

use super::presence::{Panes, Seen};
use super::prompt::{self, AtPrompt};
use super::renames::{self, Typing};
use super::{Messages, notice_of};
use crate::session::Shared;
use crate::writer::Input;

/// How long a pane must have gone without input before it is rung: long enough that a person
/// who is typing into it has paused, not left.
pub(crate) const QUIET: Duration = Duration::from_secs(3);

/// How long an idle agent's screen must have been still before it is rung. A harness that has
/// just drawn its prompt may not yet read what is typed as it will: Codex 0.154 takes a ring
/// pasted the moment its composer first appears as keys, opens its file search on the wake's
/// `@`, and never submits it (docs/observations/codex-0.154.0.md). An agent at work animates, so
/// a ring at work does not wait for this.
const STILL: Duration = Duration::from_millis(500);

/// How long an idle agent's screen may keep moving, never still for [`STILL`], before it is rung
/// regardless: an animated statusline or a clock never stops, and waiting for it would wait
/// forever. Well past a harness drawing its prompt for the first time, which is what [`STILL`]
/// waits out; the prompt, read as empty just before the ring, is the guard from then on.
const MOVING: Duration = Duration::from_secs(5);

/// When the doorbell first found each pane it would ring with its screen still moving, and when it
/// last did, by pane.
pub(crate) type Moving = HashMap<String, Noted>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Noted {
    since: Instant,
    last: Instant,
}

/// How long the thread sleeps while something is pending that no change will announce - a
/// pane whose agent is not yet found, or whose prompt holds a draft.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_secs(5);

/// How long the thread sleeps with nothing to ring, where only a post or a change in a pane,
/// which wake it, can make a difference. Bounded only so that a daemon that is stopping ends the
/// thread.
const IDLE: Duration = Duration::from_mins(1);

/// How long a rung agent has to take the ring - go to work, or read what it was rung for -
/// before Return is pressed again.
pub(crate) const ANSWER: Duration = Duration::from_secs(5);

/// How long an agent whose hooks fetch its messages is left once it is seen idle with nothing
/// fetching, before it is rung: its `Stop` hook starts as the turn ends, and its wait may connect
/// just after, which is then told instead.
const HOOK_GRACE: Duration = Duration::from_secs(2);

/// How many times Return is pressed again for one ring: enough to outlast Claude Code's start,
/// which reads what is typed only once its prompt is up.
pub(crate) const PRESSES: u8 = 6;

/// How long after a ring is typed into the prompt of an agent at work its prompt is looked at
/// again, before its Return: long enough for the screen to show what was typed.
pub(crate) const SETTLE: Duration = Duration::from_secs(1);

/// A ring its agent has not yet taken.
#[derive(Debug)]
pub(crate) struct Rung {
    wake: Wake,
    /// When its text was typed, after which anything else typed into the pane sits beside it.
    typed: Instant,
    /// When it was rung, or Return last pressed for it, or its prompt last looked at.
    at: Instant,
    presses: u8,
    /// Typed while its agent worked, which takes what it is typed as part of the running turn
    /// and may stop to ask a person at any moment.
    at_work: bool,
    /// Whether its Return has been pressed. A ring typed at work waits for a second look first.
    returned: bool,
    /// A ring typed at work and not yet returned, whose prompt something covered when it was
    /// looked at again: looked at every few seconds rather than every second, for as long as its
    /// agent stays in the pane, since a dialog can stay open for as long as its person is away.
    covered: bool,
    /// How many times a ring typed at work and not yet returned found its prompt empty: a screen
    /// that has not shown what was typed yet, until it has been looked at too often for that.
    blank: u8,
}

/// Whether a pane may be rung now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Now {
    Ring,
    /// Not until its agent is idle.
    AtIdle,
    /// Not until its agent is out of the dialog it is at: an urgent wake, which may ring an
    /// agent at work.
    Unblocked,
    /// Not until this moment, when nothing will have been typed into it for [`QUIET`].
    At(Instant),
}

/// Whether a pane may be rung now; `urgent` for a wake that may ring its agent at work, and
/// `moving` when the doorbell first found the pane's screen moving as it looked to ring it.
pub(crate) fn may_ring(seen: &Seen, now: Instant, urgent: bool, moving: Option<Instant>) -> Now {
    if let Some(until) = waits_for(seen.activity, urgent) {
        return until;
    }
    let typed = seen.input_at().map(|at| at + QUIET);
    let restless = moving.is_some_and(|since| now >= since + MOVING);
    let drawn = (seen.activity != Some(Activity::Working) && !restless)
        .then(|| seen.drawn_at().map(|at| at + STILL))
        .flatten();
    match typed.into_iter().chain(drawn).max() {
        Some(settled) if settled > now => Now::At(settled),
        _ => Now::Ring,
    }
}

/// Since when the pane's screen has been found moving as the doorbell looked to ring its idle
/// agent, noted the first time, and forgotten once it is found still or its agent at work: a
/// spinner moving through a turn says nothing about the prompt drawn as the turn ends.
pub(crate) fn moving_since(
    moving: &mut Moving,
    pane: &str,
    seen: &Seen,
    now: Instant,
) -> Option<Instant> {
    let idle = matches!(seen.activity, Some(Activity::Idle | Activity::Waiting));
    let drawing = idle && seen.drawn_at().is_some_and(|at| at + STILL > now);
    if let Some(noted) = noted(moving.get(pane).copied(), drawing, now) {
        moving.insert(pane.to_string(), noted);
        Some(noted.since)
    } else {
        moving.remove(pane);
        None
    }
}

/// A pane's screen found `drawing` now, after `before`. A note the doorbell has not renewed within
/// [`LOOK_AGAIN`] - nothing was waiting to ring the pane meanwhile, so nobody looked - says nothing
/// about now, and starts again: an old one would ring a screen drawn a moment ago at once.
fn noted(before: Option<Noted>, drawing: bool, now: Instant) -> Option<Noted> {
    if !drawing {
        return None;
    }
    let since =
        before.filter(|before| now <= before.last + LOOK_AGAIN).map_or(now, |before| before.since);
    Some(Noted { since, last: now })
}

/// What a wake waits for while its agent is doing this, or nothing when it may be rung. An agent
/// in a state nobody can read is not called blocked: what it is waiting for is to be idle.
fn waits_for(activity: Option<Activity>, urgent: bool) -> Option<Now> {
    match activity {
        Some(Activity::Idle | Activity::Waiting) => None,
        Some(Activity::Working) if urgent => None,
        Some(Activity::Blocked) if urgent => Some(Now::Unblocked),
        Some(Activity::Working | Activity::Blocked) | None => Some(Now::AtIdle),
    }
}

/// Whether a wake is for messages any of which was posted urgently.
pub(crate) fn is_urgent(wake: &Wake) -> bool {
    wake.notice.urgent > 0
}

/// What came of ringing one pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Came {
    Rang,
    /// Its agent is not at an empty prompt; the wake waits with the others.
    Waits,
    /// The pane would not take what was typed.
    Refused,
}

/// Looks at each pane and rings those whose agent is at an empty prompt, one line and then
/// Return; keeps what it rang, to press Return for again if the ring is not taken, and puts
/// back what it could not ring yet. Called with no lock held. Says what came of each wake, in
/// the order given; the caller sees that the doorbell looks again when it should.
pub(crate) fn ring_all(shared: &Shared, ringing: Vec<(Wake, Seen)>) -> Vec<Came> {
    let mut rung = Vec::new();
    let mut waiting = Vec::new();
    let mut came = Vec::new();
    for (wake, seen) in ringing {
        match prompt::look(&seen.io, &seen.agent, &shared.detecting, is_urgent(&wake)) {
            AtPrompt::Empty { at_work } => {
                let text = prompt::as_typed(&messaging::wake_text(&notice_of(&wake.notice)));
                let took = seen.io.queue(Input::Ring { text, enter: !at_work });
                rang(&wake, took);
                if took {
                    let at = Instant::now();
                    shared.messages().left.remove(pane_of(&wake));
                    rung.push(Rung {
                        wake,
                        typed: at,
                        at,
                        presses: 0,
                        at_work,
                        returned: !at_work,
                        covered: false,
                        blank: 0,
                    });
                    came.push(Came::Rang);
                } else {
                    came.push(Came::Refused);
                }
            }
            AtPrompt::Holds(held) => {
                send_left(shared, &wake, &seen, &held);
                waits(&wake, "its prompt is not empty");
                waiting.push(wake);
                came.push(Came::Waits);
            }
            AtPrompt::Not(why) => {
                waits(&wake, why);
                waiting.push(wake);
                came.push(Came::Waits);
            }
        }
    }
    if rung.is_empty() && waiting.is_empty() {
        return came;
    }
    {
        let mut messages = shared.messages();
        // A later ring for the same group stands for any earlier one.
        messages.rung.retain(|earlier| {
            !rung.iter().any(|later| {
                later.wake.name == earlier.wake.name
                    && later.wake.notice.group == earlier.wake.notice.group
            })
        });
        messages.rung.extend(rung);
        messages.pending.extend(waiting);
    }
    came
}

/// Sends a ring an earlier one gave up and left in the prompt, when the prompt holds exactly that
/// and nobody has typed into the pane since: otherwise the prompt reads as a draft for good, and
/// nothing is rung there again. The wake waiting meanwhile is rung once the prompt is empty.
fn send_left(shared: &Shared, wake: &Wake, seen: &Seen, held: &str) {
    let Via::Pane(pane) = &wake.via else { return };
    let Some((text, at)) = shared.messages().left.get(pane).cloned() else { return };
    if !prompt::is_only(held, &text) || seen.io.someone_typed_at().is_some_and(|typed| typed > at) {
        return;
    }
    if seen.io.queue(Input::Ring { text: String::new(), enter: true }) {
        shared.messages().left.remove(pane);
        log::info("msg.ring.left_sent", fields! { "name" => wake.name, "pane" => pane });
    }
}

fn waits(wake: &Wake, why: &str) {
    // Debug: a draft left in a prompt is looked at again every few seconds.
    log::debug("msg.ring.waits", fields! { "name" => wake.name, "why" => why });
}

/// The thread's handle, so a post or a change in a pane's agent can wake it.
#[derive(Debug, Default)]
pub(crate) struct Doorbell {
    thread: OnceLock<Thread>,
}

impl Doorbell {
    /// Starts the thread. Once, after the shared state is whole, since the thread reads it.
    pub(crate) fn start(&self, shared: Weak<Shared>) {
        let started =
            std::thread::Builder::new().name("doorbell".to_string()).spawn(move || run(&shared));
        match started {
            Ok(handle) => {
                let _ = self.thread.set(handle.thread().clone());
            }
            Err(error) => log::error(
                "msg.doorbell.no_thread",
                fields! {
                    "error" => error,
                    "impact" => "an agent in a pane that was busy when a message came for it is \
                                 not rung once it is idle, and is not woken again after going \
                                 idle with messages unread",
                    "check" => "whether the daemon is out of threads",
                },
            ),
        }
    }

    /// Something may be ready to ring.
    pub(crate) fn nudge(&self) {
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }
}

fn run(shared: &Weak<Shared>) {
    // What each watched pane's agent was doing when last looked at, to see it go idle.
    let mut before: HashMap<String, Option<Activity>> = HashMap::new();
    let mut typing = Typing::new();
    let mut moving = Moving::new();
    // The first look is at once: a daemon that starts may already hold wakes to ring again.
    let mut sleep = Duration::ZERO;
    loop {
        std::thread::park_timeout(sleep);
        let Some(shared) = shared.upgrade() else { return };
        sleep = look(&shared, &mut before, &mut typing, &mut moving);
    }
}

/// Types the panes' names wanted in their agents' sessions, rings what may be rung, and says how
/// long to sleep before looking again.
fn look(
    shared: &Shared,
    before: &mut HashMap<String, Option<Activity>>,
    typing: &mut Typing,
    moving: &mut Moving,
) -> Duration {
    let (quiet, handing_over) = standing(shared);
    if quiet && typing.is_empty() && !shared.lock().wants_session_names() {
        before.clear();
        return IDLE;
    }
    if handing_over {
        return LOOK_AGAIN;
    }
    let panes = Panes::of(shared);
    let mut next: Option<Instant> = None;
    // First, so that a ring looking at the same pane sees the rename's write and waits it out.
    moving.retain(|pane, _| panes.exists(pane));
    renames::look(shared, &panes, Instant::now(), typing, moving, &mut next);
    if quiet {
        before.clear();
        return until(next, false);
    }
    let now = Instant::now();
    let mut ringing: Vec<(Wake, Seen)> = Vec::new();
    let pressing;
    let mut unfound = false;
    {
        let mut messages = shared.messages();
        if messages.handing_over {
            return LOOK_AGAIN;
        }
        messages.left.retain(|pane, _| panes.exists(pane));
        went_idle(&mut messages, before, &panes, now, &mut next);
        for wake in std::mem::take(&mut messages.pending) {
            let Via::Pane(pane) = &wake.via else { continue };
            // Forgotten since: its group was paused, or it read. Resuming wakes it afresh.
            let why = if !messages.service.woken_for(&wake.name, &wake.notice.group) {
                Some("no longer woken")
            } else if messages.service.hooked(&wake.name, super::now_ms(), &panes) {
                if let Some(answered) = messages.service.hand_to_wait(&wake.name, &wake.notice) {
                    log::info(
                        "msg.ring.handed_to_wait",
                        fields! { "name" => wake.name, "group" => wake.notice.group },
                    );
                    messages.end_waits(&[answered]);
                    continue;
                }
                Some("its hooks fetch it")
            } else if let Some(until) =
                messages.hook_grace.get(&wake.name).copied().filter(|until| *until > now)
            {
                sooner(&mut next, until);
                messages.pending.push(wake);
                continue;
            } else {
                None
            };
            if let Some(why) = why {
                log::info(
                    "msg.ring.dropped",
                    fields! { "name" => wake.name, "pane" => pane, "why" => why },
                );
                continue;
            }
            let dropped = match (panes.get(pane), panes.doorbell(pane)) {
                (Some(seen), Ringable::Rings) => {
                    let since = moving_since(moving, pane, seen, now);
                    match may_ring(seen, now, is_urgent(&wake), since) {
                        Now::Ring => ringing.push((wake, seen.clone())),
                        Now::At(at) => {
                            sooner(&mut next, at);
                            messages.pending.push(wake);
                        }
                        Now::AtIdle | Now::Unblocked => messages.pending.push(wake),
                    }
                    continue;
                }
                (_, Ringable::AgentToCome) => {
                    unfound = true;
                    messages.pending.push(wake);
                    continue;
                }
                (_, Ringable::NoPrompt) => "its agent's prompt cannot be read",
                (_, Ringable::Rings | Ringable::NoAgent) if panes.exists(pane) => {
                    "no agent is in its pane, and none is coming"
                }
                _ => "the pane closed",
            };
            log::info(
                "msg.ring.dropped",
                fields! { "name" => wake.name, "pane" => pane, "why" => dropped },
            );
        }
        pressing = unanswered_rings(&mut messages, &panes, now, &mut next);
    }
    let came = ring_all(shared, ringing);
    // A ring is looked at again once it is due its Return, or a Return pressed again; nothing
    // announces a draft being cleared, so a prompt that held one is looked at again too.
    for (what, after) in [(Came::Rang, SETTLE), (Came::Waits, LOOK_AGAIN)] {
        if came.contains(&what) {
            sooner(&mut next, Instant::now() + after);
        }
    }
    if press_again(shared, pressing) {
        sooner(&mut next, Instant::now() + ANSWER);
    }
    until(next, unfound)
}

/// Whether no wake is waiting to be rung, pressed again or woken once more, and whether a handoff
/// is under way.
fn standing(shared: &Shared) -> (bool, bool) {
    let messages = shared.messages();
    let quiet = messages.pending.is_empty()
        && messages.rung.is_empty()
        && messages.service.watched().is_empty();
    (quiet, messages.handing_over)
}

/// How long to sleep before looking again at `next`, or with nothing due, at an agent
/// `unfound` that may yet come to a pane.
fn until(next: Option<Instant>, unfound: bool) -> Duration {
    match next {
        Some(next) => next.saturating_duration_since(Instant::now()).min(LOOK_AGAIN),
        None if unfound => LOOK_AGAIN,
        None => IDLE,
    }
}

/// Wakes once more each agent that went idle since the last look with what it was woken for
/// still unread, holding back one whose hooks fetch its messages for [`HOOK_GRACE`].
fn went_idle(
    messages: &mut Messages,
    before: &mut HashMap<String, Option<Activity>>,
    panes: &Panes,
    now: Instant,
    next: &mut Option<Instant>,
) {
    messages.hook_grace.retain(|_, until| *until > now);
    let watched = messages.service.watched();
    before.retain(|pane, _| watched.iter().any(|(_, watched)| watched == pane));
    for (name, pane) in watched {
        let activity = panes.get(&pane).and_then(|seen| seen.activity);
        let was = before.insert(pane, activity).flatten();
        let went_idle = matches!(was, Some(Activity::Working | Activity::Blocked))
            && activity == Some(Activity::Idle);
        // Idle after the wake, not before it: one still waiting to be rung is not late.
        let unrung = messages.pending.iter().any(|wake| wake.name == name);
        if !went_idle || unrung {
            continue;
        }
        let (wakes, unsaved) = messages.service.went_idle(&name, panes, super::now_ms());
        if let Some(error) = unsaved {
            super::kept_nothing(&muster_msg::Refusal::Store { error });
        }
        if !wakes.is_empty() && messages.service.fetches_with_hooks(&name) {
            messages.hook_grace.insert(name.clone(), now + HOOK_GRACE);
            sooner(next, now + HOOK_GRACE);
        }
        messages.pending.extend(wakes);
    }
}

/// Keeps the rings not yet taken, and takes out those due their Return, or a Return pressed
/// again. A ring whose agent went to work, or read, was taken; one pressed as often as it may be
/// ends. An urgent ring may have been typed at work, so going to work says nothing about it: its
/// agent's prompt is looked at once it is due, and an empty one means it was taken. One whose
/// Return is still to come is not taken by anything its agent does meanwhile, a dialog least of
/// all: only its prompt holding it can say what the Return would send.
fn unanswered_rings(
    messages: &mut Messages,
    panes: &Panes,
    now: Instant,
    next: &mut Option<Instant>,
) -> Vec<(Rung, Seen)> {
    let mut pressing = Vec::new();
    for rung in std::mem::take(&mut messages.rung) {
        let Via::Pane(pane) = &rung.wake.via else { continue };
        let Some(seen) = panes.get(pane) else { continue };
        let urgent = is_urgent(&rung.wake);
        let taken = (rung.returned && waits_for(seen.activity, urgent).is_some())
            || !messages.service.woken_for(&rung.wake.name, &rung.wake.notice.group);
        if taken {
            continue;
        }
        let due = rung.at + if rung.returned || rung.covered { ANSWER } else { SETTLE };
        if rung.returned && rung.presses >= PRESSES && due <= now {
            ended(messages, &rung, "Return was pressed for it as often as it may be");
            continue;
        }
        // A ring's own typing starts the quiet period; before its Return, anybody else's ends it
        // ([`press_again`]), so it is not waited out.
        let settled = if rung.returned { may_ring(seen, now, urgent, None) } else { Now::Ring };
        let at = if due > now {
            due
        } else if let Now::At(quiet) = settled {
            quiet
        } else {
            pressing.push((rung, seen.clone()));
            continue;
        };
        sooner(next, at);
        messages.rung.push(rung);
    }
    pressing
}

/// Presses Return for each ring whose text sits unsent in its agent's prompt, and nothing else
/// there, which is all such a Return can send: a ring typed at work, due its first, or one
/// whose Return was not taken. A prompt holding anything more, or not showing at all, ends the
/// ring; an empty one means the ring was sent, or cleared. A ring typed at work and not yet
/// returned is kept instead while its prompt does not show: a dialog that opened over it may
/// close again. Called with no lock held; true when any was pressed or kept.
fn press_again(shared: &Shared, pressing: Vec<(Rung, Seen)>) -> bool {
    let mut pressed = Vec::new();
    let mut ending: Vec<(Rung, &'static str)> = Vec::new();
    for (mut rung, seen) in pressing {
        // Before the screen, which can lag what was typed: an agent slow to paint, or this
        // daemon's copy behind, shows the ring alone over words that a Return would send.
        if seen.io.someone_typed_at().is_some_and(|typed| typed > rung.typed) {
            ending.push((rung, "something was typed into its pane after it was rung"));
            continue;
        }
        let text = messaging::wake_text(&notice_of(&rung.wake.notice));
        match prompt::look(&seen.io, &seen.agent, &shared.detecting, is_urgent(&rung.wake)) {
            AtPrompt::Holds(held) if prompt::is_only(&held, &text) => {
                if !seen.io.queue(Input::Ring { text: String::new(), enter: true }) {
                    ending.push((rung, "its pane would not take the Return"));
                    continue;
                }
                let event = if rung.returned {
                    rung.presses += 1;
                    "msg.ring.pressed_again"
                } else {
                    "msg.ring.returned"
                };
                rung.returned = true;
                rung.at = Instant::now();
                log::info(
                    event,
                    fields! {
                        "name" => rung.wake.name,
                        "group" => rung.wake.notice.group,
                        "presses" => rung.presses,
                    },
                );
                pressed.push(rung);
            }
            AtPrompt::Empty { .. } if !rung.returned && rung.blank < PRESSES => {
                rung.blank += 1;
                rung.at = Instant::now();
                pressed.push(rung);
            }
            AtPrompt::Empty { .. } if !rung.returned => {
                ending.push((rung, "its prompt stayed empty, and its Return was never pressed"));
            }
            AtPrompt::Empty { .. } => {}
            AtPrompt::Holds(_) => ending.push((rung, "its prompt holds more than the ring")),
            AtPrompt::Not(_) if !rung.returned => {
                rung.covered = true;
                rung.at = Instant::now();
                pressed.push(rung);
            }
            AtPrompt::Not(why) => ending.push((rung, why)),
        }
    }
    if pressed.is_empty() && ending.is_empty() {
        return false;
    }
    let any = !pressed.is_empty();
    let mut messages = shared.messages();
    messages.rung.extend(pressed);
    for (rung, why) in &ending {
        ended(&mut messages, rung, why);
    }
    any
}

/// Gives up a ring its agent never took, and forgets that the agent was woken for it, so the
/// next post there rings again rather than counting it woken. A ring typed at work is given up
/// and the agent still counted woken: whatever ended it - a person typing, a dialog, a screen
/// pushed aside - came while the agent works, and it is woken once more as it goes idle with the
/// message unread, which forgetting would lose.
fn ended(messages: &mut Messages, rung: &Rung, why: &str) {
    if let Via::Pane(pane) = &rung.wake.via {
        let text = messaging::wake_text(&notice_of(&rung.wake.notice));
        messages.left.insert(pane.clone(), (text, rung.typed));
    }
    if rung.at_work {
        log::info(
            "msg.ring.left",
            fields! {
                "name" => rung.wake.name,
                "group" => rung.wake.notice.group,
                "returned" => rung.returned,
                "why" => why,
            },
        );
        return;
    }
    log::warn(
        "msg.ring.ended",
        fields! {
            "name" => rung.wake.name,
            "group" => rung.wake.notice.group,
            "presses" => rung.presses,
            "why" => why,
            "impact" => "the agent was rung and has neither gone to work nor read; it is not \
                         rung again until another post comes for it",
            "check" => "what the pane shows: the wake may sit unsent in the agent's prompt \
                        beside something typed after it",
        },
    );
    if let Some(error) = messages.service.unwake(&rung.wake.name, &rung.wake.notice.group) {
        super::kept_nothing(&muster_msg::Refusal::Store { error });
    }
}

pub(crate) fn sooner(next: &mut Option<Instant>, at: Instant) {
    *next = Some(next.map_or(at, |next| next.min(at)));
}

fn pane_of(wake: &Wake) -> &str {
    match &wake.via {
        Via::Pane(pane) => pane.as_str(),
        Via::Inbox(inbox) => inbox.socket.as_str(),
        Via::Human => muster_msg::HUMAN,
    }
}

/// Says what came of a ring.
pub(crate) fn rang(wake: &Wake, took: bool) {
    let pane = pane_of(wake);
    if took {
        log::info(
            "msg.rang",
            fields! {
                "name" => wake.name,
                "pane" => pane,
                "group" => wake.notice.group,
                "again" => wake.notice.again,
            },
        );
    } else {
        log::warn(
            "msg.ring.failed",
            fields! {
                "name" => wake.name,
                "pane" => pane,
                "impact" => "the agent is not woken for these messages until it reads or \
                             goes idle again",
                "check" => "whether the pane's program has stopped reading its terminal, \
                            with its input queue full",
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_urgent_wake_waits_for_a_blocked_agent_to_be_unblocked_and_for_an_unread_one_to_be_idle() {
        assert_eq!(waits_for(Some(Activity::Blocked), true), Some(Now::Unblocked));
        assert_eq!(waits_for(None, true), Some(Now::AtIdle));
        assert_eq!(waits_for(Some(Activity::Working), true), None);
    }

    #[test]
    fn a_screen_is_moving_since_it_was_first_found_so_and_a_stale_note_starts_again() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let first = noted(None, true, at(0)).unwrap();
        let renewed = noted(Some(first), true, at(4)).unwrap();
        assert_eq!(renewed.since, at(0), "noted again within a look");
        assert_eq!(
            noted(Some(renewed), true, at(60)).unwrap().since,
            at(60),
            "nobody looked meanwhile"
        );
        assert_eq!(noted(Some(renewed), false, at(5)), None, "found still");
    }

    #[test]
    fn an_ordinary_wake_waits_for_idle_whatever_its_agent_is_doing() {
        for activity in [Some(Activity::Working), Some(Activity::Blocked), None] {
            assert_eq!(waits_for(activity, false), Some(Now::AtIdle), "{activity:?}");
        }
        assert_eq!(waits_for(Some(Activity::Idle), false), None);
        assert_eq!(waits_for(Some(Activity::Waiting), false), None);
    }
}
