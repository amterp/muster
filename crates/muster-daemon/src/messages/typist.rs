//! A line typed at an agent's idle, empty prompt on Muster's behalf - a pane's name for its
//! session ([`super::renames`]), a compaction ([`super::compacts`]) - and seen through until the
//! agent takes it. Typed by the doorbell's thread, the one thread that types into prompts, so
//! such a line and a ring never land in a prompt together.
//!
//! It is typed as a ring at an idle prompt is, under the same guards ([`super::doorbell`]): the
//! agent idle or waiting, nothing typed into the pane for a few seconds nor drawn there for half
//! a second, and its prompt read as empty just before, one line and its Return in one write. It
//! is never typed at work, and never into a pane something else is reaching meanwhile: a line can
//! wait for the turn to end, and a Return typed at work can answer a dialog that opened meanwhile.
//!
//! It is taken once the prompt is empty again. One left unsent in the prompt, as Claude Code
//! leaves what is typed while it starts, has Return pressed again while the prompt holds it and
//! nothing else. Anything else gives it up, and it is not typed again until it is wanted afresh:
//! retyping a line into a screen that would not take it once is how a loop starts.

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

/// One kind of line, and what becomes of it.
pub(crate) trait Line {
    /// What the line is for, kept until it is taken or given up: a name, the line itself.
    type What: Clone;

    /// What a give-up reason calls the line: "its prompt holds more than the rename".
    const NOUN: &'static str;

    /// The log event for the line typed.
    const TYPED: &'static str;

    /// Whether the agent going to work after the line counts as taking it, as it does for a line
    /// that starts a turn of its own.
    const TAKEN_AT_WORK: bool;

    /// The line still to be typed into the pane's agent, if one is, and what it is for.
    fn wanted(seen: &Seen) -> Option<(Self::What, &str)>;

    /// Whether a line typed earlier still matters. One that no longer does is let go without a
    /// word: it was seen through some other way.
    fn still_wanted(seen: &Seen) -> bool;

    /// The agent took it.
    fn taken(shared: &Shared, pane: &str, what: &Self::What);

    /// Typing it was given up, for `why`.
    fn given_up(shared: &Shared, pane: &str, what: &Self::What, why: &str);
}

/// A line typed and not yet seen taken, by pane.
pub(crate) type Typing<W> = HashMap<String, Typed<W>>;

#[derive(Debug)]
pub(crate) struct Typed<W> {
    what: W,
    line: String,
    /// When it was typed, or Return last pressed for it.
    at: Instant,
    presses: u8,
}

/// Types each pane's line still wanted into its agent's prompt where the pane allows, and looks
/// again at each typed earlier, leaving alone the panes `busy` names, where something else is
/// handing the agent a line. Says through `next` when it wants to look again. Called with no lock
/// held.
pub(crate) fn look<L: Line>(
    shared: &Shared,
    panes: &Panes,
    busy: &HashSet<String>,
    now: Instant,
    typing: &mut Typing<L::What>,
    moving: &mut Moving,
    next: &mut Option<Instant>,
) {
    second_looks::<L>(shared, panes, now, typing, next);
    for (pane, seen) in panes.iter() {
        let Some((what, line)) = L::wanted(seen) else { continue };
        if typing.contains_key(pane) || busy.contains(pane) {
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
            L::given_up(shared, pane, &what, "its pane would not take what was typed");
            continue;
        }
        log::info(L::TYPED, fields! { "pane" => pane, "agent" => seen.agent });
        let at = Instant::now();
        let line = line.to_string();
        typing.insert(pane.clone(), Typed { what, line, at, presses: 0 });
        doorbell::sooner(next, at + doorbell::SETTLE);
    }
}

/// Looks again at each line typed, once the screen has had time to show what came of it.
fn second_looks<L: Line>(
    shared: &Shared,
    panes: &Panes,
    now: Instant,
    typing: &mut Typing<L::What>,
    next: &mut Option<Instant>,
) {
    let mut ended = Vec::new();
    typing.retain(|pane, typed| {
        // Gone with its agent, or seen through some other way. A line wanted again meanwhile
        // still has this one seen through first, since it may sit unsent in the prompt, where the
        // next would wait on it for good.
        let Some(seen) = panes.get(pane) else { return false };
        if !L::still_wanted(seen) {
            return false;
        }
        let due = typed.at + doorbell::SETTLE;
        if due > now {
            doorbell::sooner(next, due);
            return true;
        }
        match second_look::<L>(shared, seen, typed) {
            Second::Taken => {
                ended.push((pane.clone(), typed.what.clone(), None));
                false
            }
            Second::Pressed => {
                doorbell::sooner(next, typed.at + doorbell::ANSWER);
                true
            }
            Second::Ended(why) => {
                ended.push((pane.clone(), typed.what.clone(), Some(why)));
                false
            }
        }
    });
    for (pane, what, why) in ended {
        match why {
            None => L::taken(shared, &pane, &what),
            Some(why) => L::given_up(shared, &pane, &what, &why),
        }
    }
}

enum Second {
    Taken,
    Pressed,
    Ended(String),
}

fn second_look<L: Line>(shared: &Shared, seen: &Seen, typed: &mut Typed<L::What>) -> Second {
    if seen.io.someone_typed_at().is_some_and(|at| at > typed.at) {
        return Second::Ended("something was typed into its pane after it".to_string());
    }
    if L::TAKEN_AT_WORK && !matches!(seen.activity, Some(Activity::Idle | Activity::Waiting)) {
        return Second::Taken;
    }
    match prompt::look(&seen.io, &seen.agent, &shared.detecting, false) {
        AtPrompt::Empty { .. } => Second::Taken,
        AtPrompt::Holds(held) if prompt::is_only(&held, &typed.line) => {
            if typed.presses >= doorbell::PRESSES {
                return Second::Ended("Return was pressed for it as often as it may be".into());
            }
            if !seen.io.queue(Input::Ring { text: String::new(), enter: true }) {
                return Second::Ended("its pane would not take the Return".to_string());
            }
            typed.presses += 1;
            typed.at = Instant::now();
            Second::Pressed
        }
        AtPrompt::Holds(_) => Second::Ended(format!("its prompt holds more than the {}", L::NOUN)),
        AtPrompt::Not(why) => Second::Ended(why.to_string()),
    }
}
