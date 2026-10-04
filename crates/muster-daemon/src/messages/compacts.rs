//! An agent's context compacted at its prompt (MIP-5, section 10): the line the agent's manifest
//! gives in `[session] compact`, typed by the doorbell's thread, the one thread that types into
//! prompts, so a compaction, a rename and a ring never land in a prompt together.
//!
//! It is typed as a rename is ([`super::renames`]): the agent idle or waiting, nothing typed into
//! the pane for a few seconds nor drawn there for half a second, and its prompt read as empty
//! just before, one line and its Return in one write. Never at work, where a Return can answer a
//! dialog that opened meanwhile; a compaction asked for mid-turn waits for the turn to end, which
//! is also when a harness could act on it.
//!
//! It is taken once the prompt is empty again or the agent has gone to work on it, which a
//! compaction does and a rename does not. One left unsent in the prompt has Return pressed again
//! while the prompt holds it and nothing else. Anything else gives it up, and it is not typed
//! again until somebody asks again or the context crosses the threshold again.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use muster_core::diagnostics::log;
use muster_core::fields;
use muster_msg::Activity;

use super::doorbell::{self, Moving, Now};
use super::presence::{Panes, Seen};
use super::prompt::{self, AtPrompt};
use crate::session::Shared;
use crate::writer::Input;

/// A compaction typed and not yet seen taken, by pane.
pub(crate) type Compacting = HashMap<String, Typed>;

#[derive(Debug)]
pub(crate) struct Typed {
    line: String,
    /// When it was typed, or Return last pressed for it.
    at: Instant,
    presses: u8,
}

/// Types each pane's compaction still wanted into its agent's prompt where the pane allows, and
/// looks again at each typed earlier. Skips the panes in `busy`, which something else is
/// reaching meanwhile. Says through `next` when it wants to look again. Called with no lock held.
pub(crate) fn look(
    shared: &Shared,
    panes: &Panes,
    busy: &HashSet<String>,
    now: Instant,
    compacting: &mut Compacting,
    moving: &mut Moving,
    next: &mut Option<Instant>,
) {
    second_looks(shared, panes, now, compacting, next);
    for (pane, seen) in panes.iter() {
        let Some(line) = &seen.compact else { continue };
        if compacting.contains_key(pane) || busy.contains(pane) {
            continue;
        }
        let since = doorbell::moving_since(moving, pane, seen, now);
        match doorbell::may_ring(seen, now, false, since) {
            Now::Ring => {}
            Now::At(at) => {
                doorbell::sooner(next, at);
                continue;
            }
            Now::AtIdle | Now::Unblocked => continue,
        }
        match prompt::look(&seen.io, &seen.agent, &shared.detecting, false) {
            AtPrompt::Empty { .. } => {}
            AtPrompt::Holds(_) | AtPrompt::Not(_) => {
                doorbell::sooner(next, Instant::now() + doorbell::LOOK_AGAIN);
                continue;
            }
        }
        if !seen.io.queue(Input::Ring { text: prompt::as_typed(line), enter: true }) {
            given_up(shared, pane, line, "its pane would not take what was typed");
            continue;
        }
        log::info("daemon.compact.typed", fields! { "pane" => pane, "agent" => seen.agent });
        let at = Instant::now();
        compacting.insert(pane.clone(), Typed { line: line.clone(), at, presses: 0 });
        doorbell::sooner(next, at + doorbell::SETTLE);
    }
}

/// Looks again at each compaction typed, once the screen has had time to show what came of it.
fn second_looks(
    shared: &Shared,
    panes: &Panes,
    now: Instant,
    compacting: &mut Compacting,
    next: &mut Option<Instant>,
) {
    let mut ended = Vec::new();
    compacting.retain(|pane, typed| {
        let Some(seen) = panes.get(pane) else { return false };
        let due = typed.at + doorbell::SETTLE;
        if due > now {
            doorbell::sooner(next, due);
            return true;
        }
        match second_look(shared, seen, typed) {
            Second::Taken => {
                ended.push((pane.clone(), typed.line.clone(), None));
                false
            }
            Second::Pressed => {
                doorbell::sooner(next, typed.at + doorbell::ANSWER);
                true
            }
            Second::Ended(why) => {
                ended.push((pane.clone(), typed.line.clone(), Some(why)));
                false
            }
        }
    });
    for (pane, line, why) in ended {
        match why {
            None => {
                log::info("daemon.compact.taken", fields! { "pane" => pane });
                shared.lock().compaction_typed(&pane, &line);
            }
            Some(why) => given_up(shared, &pane, &line, why),
        }
    }
}

enum Second {
    Taken,
    Pressed,
    Ended(&'static str),
}

fn second_look(shared: &Shared, seen: &Seen, typed: &mut Typed) -> Second {
    if seen.io.someone_typed_at().is_some_and(|at| at > typed.at) {
        return Second::Ended("something was typed into its pane after it");
    }
    // Compacting is a turn of its own: an agent no longer idle took the line.
    if !matches!(seen.activity, Some(Activity::Idle | Activity::Waiting)) {
        return Second::Taken;
    }
    match prompt::look(&seen.io, &seen.agent, &shared.detecting, false) {
        AtPrompt::Empty { .. } => Second::Taken,
        AtPrompt::Holds(held) if prompt::is_only(&held, &typed.line) => {
            if typed.presses >= doorbell::PRESSES {
                return Second::Ended("Return was pressed for it as often as it may be");
            }
            if !seen.io.queue(Input::Ring { text: String::new(), enter: true }) {
                return Second::Ended("its pane would not take the Return");
            }
            typed.presses += 1;
            typed.at = Instant::now();
            Second::Pressed
        }
        AtPrompt::Holds(_) => Second::Ended("its prompt holds more than the compaction"),
        AtPrompt::Not(why) => Second::Ended(why),
    }
}

/// Gives up typing a compaction, which is then not typed again until it is asked for again.
fn given_up(shared: &Shared, pane: &str, line: &str, why: &str) {
    log::warn(
        "daemon.compact.given_up",
        fields! {
            "pane" => pane,
            "why" => why,
            "impact" => "the agent's context is not compacted; nothing types the compaction again \
                         until somebody asks again or its context crosses compact_at again",
            "check" => "what the pane shows: the compaction may sit in the agent's prompt beside \
                        something typed after it, or a dialog may have opened over it; \
                        `muster pane compact` asks again",
        },
    );
    shared.lock().compaction_typed(pane, line);
}
