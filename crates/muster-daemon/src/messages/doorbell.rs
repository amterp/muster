//! The doorbell (MIP-4, section 6): a wake typed into the pane an agent runs in, as one line
//! and a Return, for an agent nothing else can wake - which, since a session that bypasses
//! permission prompts holds what its inbox is sent, is every agent in a pane.
//!
//! A Return typed at the wrong moment is an answer, so a pane is rung only while its agent is
//! idle or waiting and nothing has been typed into it for a few seconds. Blocked is a dialog the
//! Return would answer. Working is ruled out too, because a dialog can open between the check
//! and the write. What cannot be rung at once waits here, and this thread rings it when the pane
//! allows. The same thread wakes an agent once more when it goes idle with what it was woken
//! for still unread (section 5).

use std::collections::HashMap;
use std::sync::{OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_daemon_proto::messaging;
use muster_msg::{Activity, Via, Wake};

use super::notice_of;
use super::presence::{Panes, Seen};
use crate::session::Shared;
use crate::writer::Input;

/// How long a pane must have gone without input before it is rung: long enough that a person
/// who is typing into it has paused, not left.
pub(crate) const QUIET: Duration = Duration::from_secs(3);

/// How long the thread sleeps while something is pending that no change will announce - a
/// pane whose agent is not yet found, say.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

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

/// Types the wake into the pane: one line, then Return. False when the pane would not take it.
pub(crate) fn ring(seen: &Seen, wake: &Wake) -> bool {
    let text = messaging::wake_text(&notice_of(&wake.notice));
    seen.io.queue(Input::Send { text, enter: true })
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
    let mut sleep: Option<Duration> = None;
    loop {
        match sleep {
            Some(duration) => std::thread::park_timeout(duration),
            None => std::thread::park(),
        }
        let Some(shared) = shared.upgrade() else { return };
        sleep = look(&shared, &mut before);
    }
}

/// Rings what may be rung, and says how long to sleep before looking again: nothing when only a
/// post or a change in a pane can make a difference.
fn look(shared: &Shared, before: &mut HashMap<String, Option<Activity>>) -> Option<Duration> {
    {
        let messages = shared.messages();
        if messages.pending.is_empty() && messages.service.watched().is_empty() {
            before.clear();
            return None;
        }
    }
    let panes = Panes::of(shared);
    let now = Instant::now();
    let mut ringing: Vec<(Wake, Seen)> = Vec::new();
    let mut next: Option<Instant> = None;
    let mut unfound = false;
    {
        let mut messages = shared.messages();
        if messages.handing_over {
            return Some(LOOK_AGAIN);
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
            match panes.get(pane) {
                Some(seen) => match may_ring(seen, now) {
                    Now::Ring => ringing.push((wake, seen.clone())),
                    Now::At(at) => {
                        next = Some(next.map_or(at, |next| next.min(at)));
                        messages.pending.push(wake);
                    }
                    Now::AtIdle => messages.pending.push(wake),
                },
                None if panes.exists(pane) => {
                    unfound = true;
                    messages.pending.push(wake);
                }
                None => log::info(
                    "msg.ring.dropped",
                    fields! { "name" => wake.name, "pane" => pane, "why" => "the pane closed" },
                ),
            }
        }
    }
    for (wake, seen) in &ringing {
        rang(wake, ring(seen, wake));
    }
    let until_quiet = next.map(|next| next.saturating_duration_since(Instant::now()));
    match until_quiet {
        Some(duration) => Some(duration.min(LOOK_AGAIN)),
        None if unfound => Some(LOOK_AGAIN),
        None => None,
    }
}

/// Says what came of a ring.
pub(crate) fn rang(wake: &Wake, took: bool) {
    let pane = match &wake.via {
        Via::Pane(pane) => pane.as_str(),
        Via::Inbox(inbox) => inbox.socket.as_str(),
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
