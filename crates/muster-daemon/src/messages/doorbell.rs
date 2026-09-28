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
//! Claude Code keeps what is typed while it starts as its prompt and drops the Return, so a
//! ring can sit unsent. Return is pressed again for it, a few times, but only while nobody has
//! typed into the pane since and the prompt holds the ring's own text and nothing else: that
//! Return can send nothing but the ring. The first is what the daemon saw written, since the
//! screen can lag it.

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
use super::{Messages, notice_of};
use crate::session::Shared;
use crate::writer::Input;

/// How long a pane must have gone without input before it is rung: long enough that a person
/// who is typing into it has paused, not left.
pub(crate) const QUIET: Duration = Duration::from_secs(3);

/// How long the thread sleeps while something is pending that no change will announce - a
/// pane whose agent is not yet found, or whose prompt holds a draft.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

/// How long the thread sleeps with nothing to ring, where only a post or a change in a pane,
/// which wake it, can make a difference. Bounded only so that a daemon that is stopping ends the
/// thread.
const IDLE: Duration = Duration::from_mins(1);

/// How long a rung agent has to take the ring - go to work, or read what it was rung for -
/// before Return is pressed again.
const ANSWER: Duration = Duration::from_secs(5);

/// How many times Return is pressed again for one ring: enough to outlast Claude Code's start,
/// which reads what is typed only once its prompt is up.
const PRESSES: u8 = 6;

/// A ring its agent has not yet taken.
#[derive(Debug)]
pub(crate) struct Rung {
    wake: Wake,
    /// When it was rung, or Return last pressed for it.
    at: Instant,
    presses: u8,
}

/// Whether a pane may be rung now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Now {
    Ring,
    /// Not until its agent is idle.
    AtIdle,
    /// Not until this moment, when nothing will have been typed into it for [`QUIET`].
    At(Instant),
}

pub(crate) fn may_ring(seen: &Seen, now: Instant) -> Now {
    if !matches!(seen.activity, Some(Activity::Idle | Activity::Waiting)) {
        return Now::AtIdle;
    }
    match seen.input_at().map(|at| at + QUIET) {
        Some(quiet) if quiet > now => Now::At(quiet),
        _ => Now::Ring,
    }
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
        match prompt::look(&seen.io, &seen.agent, &shared.detecting) {
            AtPrompt::Empty => {
                let text = messaging::wake_text(&notice_of(&wake.notice));
                let took = seen.io.queue(Input::Ring { text, enter: true });
                rang(&wake, took);
                if took {
                    rung.push(Rung { wake, at: Instant::now(), presses: 0 });
                    came.push(Came::Rang);
                } else {
                    came.push(Came::Refused);
                }
            }
            AtPrompt::Holds(_) => {
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
    // The first look is at once: a daemon that starts may already hold wakes to ring again.
    let mut sleep = Duration::ZERO;
    loop {
        std::thread::park_timeout(sleep);
        let Some(shared) = shared.upgrade() else { return };
        sleep = look(&shared, &mut before);
    }
}

/// Rings what may be rung, and says how long to sleep before looking again.
fn look(shared: &Shared, before: &mut HashMap<String, Option<Activity>>) -> Duration {
    {
        let messages = shared.messages();
        if messages.pending.is_empty()
            && messages.rung.is_empty()
            && messages.service.watched().is_empty()
        {
            before.clear();
            return IDLE;
        }
    }
    let panes = Panes::of(shared);
    let now = Instant::now();
    let mut ringing: Vec<(Wake, Seen)> = Vec::new();
    let pressing;
    let mut next: Option<Instant> = None;
    let mut unfound = false;
    {
        let mut messages = shared.messages();
        if messages.handing_over {
            return LOOK_AGAIN;
        }
        let watched = messages.service.watched();
        before.retain(|pane, _| watched.iter().any(|(_, watched)| watched == pane));
        for (name, pane) in watched {
            let activity = panes.get(&pane).and_then(|seen| seen.activity);
            let was = before.insert(pane, activity).flatten();
            let went_idle = matches!(was, Some(Activity::Working | Activity::Blocked))
                && activity == Some(Activity::Idle);
            // Idle after the wake, not before it: one still waiting to be rung is not late.
            let unrung = messages.pending.iter().any(|wake| wake.name == name);
            if went_idle && !unrung {
                let (wakes, unsaved) = messages.service.went_idle(&name, &panes);
                if let Some(error) = unsaved {
                    super::kept_nothing(&muster_msg::Refusal::Store { error });
                }
                messages.pending.extend(wakes);
            }
        }
        for wake in std::mem::take(&mut messages.pending) {
            let Via::Pane(pane) = &wake.via else { continue };
            let dropped = match (panes.get(pane), panes.doorbell(pane)) {
                (Some(seen), Ringable::Rings) => {
                    match may_ring(seen, now) {
                        Now::Ring => ringing.push((wake, seen.clone())),
                        Now::At(at) => {
                            sooner(&mut next, at);
                            messages.pending.push(wake);
                        }
                        Now::AtIdle => messages.pending.push(wake),
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
    // A ring is looked at again once it is due a Return pressed again; nothing announces a draft
    // being cleared, so a prompt that held one is looked at again too.
    if came.contains(&Came::Rang) {
        sooner(&mut next, Instant::now() + ANSWER);
    }
    if came.contains(&Came::Waits) {
        sooner(&mut next, Instant::now() + LOOK_AGAIN);
    }
    if press_again(shared, pressing) {
        sooner(&mut next, Instant::now() + ANSWER);
    }
    match next {
        Some(next) => next.saturating_duration_since(Instant::now()).min(LOOK_AGAIN),
        None if unfound => LOOK_AGAIN,
        None => IDLE,
    }
}

/// Keeps the rings not yet taken, and takes out those due a Return pressed again. A ring whose
/// agent went to work, or read, was taken; one pressed as often as it may be ends.
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
        let taken = !matches!(seen.activity, Some(Activity::Idle | Activity::Waiting))
            || !messages.service.woken_for(&rung.wake.name, &rung.wake.notice.group);
        if taken {
            continue;
        }
        let due = rung.at + ANSWER;
        if rung.presses >= PRESSES && due <= now {
            ended(messages, &rung, "Return was pressed for it as often as it may be");
            continue;
        }
        let at = if due > now {
            due
        } else if let Now::At(quiet) = may_ring(seen, now) {
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

/// Presses Return again for each ring whose text sits unsent in its agent's prompt, and nothing
/// else there, which is all such a Return can send. A prompt holding anything more, or not
/// showing at all, ends the ring; an empty one means the ring was sent, or cleared. Called with
/// no lock held; true when any was pressed.
fn press_again(shared: &Shared, pressing: Vec<(Rung, Seen)>) -> bool {
    let mut pressed = Vec::new();
    let mut ending: Vec<(Rung, &'static str)> = Vec::new();
    for (mut rung, seen) in pressing {
        // Before the screen, which can lag what was typed: an agent slow to paint, or this
        // daemon's copy behind, shows the ring alone over words that a Return would send.
        if seen.io.someone_typed_at().is_some_and(|typed| typed > rung.at) {
            ending.push((rung, "something was typed into its pane after it was rung"));
            continue;
        }
        let text = messaging::wake_text(&notice_of(&rung.wake.notice));
        match prompt::look(&seen.io, &seen.agent, &shared.detecting) {
            AtPrompt::Holds(held) if prompt::is_only(&held, &text) => {
                if !seen.io.queue(Input::Ring { text: String::new(), enter: true }) {
                    ending.push((rung, "its pane would not take the Return"));
                    continue;
                }
                rung.presses += 1;
                rung.at = Instant::now();
                log::info(
                    "msg.ring.pressed_again",
                    fields! {
                        "name" => rung.wake.name,
                        "group" => rung.wake.notice.group,
                        "presses" => rung.presses,
                    },
                );
                pressed.push(rung);
            }
            AtPrompt::Empty => {}
            AtPrompt::Holds(_) => ending.push((rung, "its prompt holds more than the ring")),
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
/// next post there rings again rather than counting it woken.
fn ended(messages: &mut Messages, rung: &Rung, why: &str) {
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

fn sooner(next: &mut Option<Instant>, at: Instant) {
    *next = Some(next.map_or(at, |next| next.min(at)));
}

/// Says what came of a ring.
pub(crate) fn rang(wake: &Wake, took: bool) {
    let pane = match &wake.via {
        Via::Pane(pane) => pane.as_str(),
        Via::Inbox(inbox) => inbox.socket.as_str(),
        Via::Human => muster_msg::HUMAN,
    };
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
