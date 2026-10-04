//! A pane's name typed into its agent's session (MIP-5, section 10): the line the agent's
//! manifest gives in `[session] rename`, typed by the doorbell's thread, the one thread that
//! types into prompts, so a rename and a ring never land in a prompt together.
//!
//! It is typed as a ring at an idle prompt is, under the same guards ([`super::doorbell`]): the
//! agent idle or waiting, nothing typed into the pane for a few seconds nor drawn there for half
//! a second, and its prompt read as empty just before, one line and its Return in one write. It
//! is never typed at work. A rename can wait for the turn to end, and a Return typed at work can
//! answer a dialog that opened meanwhile.
//!
//! A rename is taken once the prompt is empty again, or once the harness reports the name. One
//! left unsent in the prompt, as Claude Code leaves what is typed while it starts, has Return
//! pressed again while the prompt holds it and nothing else. Anything else ends it, and the name
//! is not typed again until the pane is renamed: retyping it into a screen that would not take
//! it once is how a loop starts.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use muster_core::diagnostics::log;
use muster_core::fields;

use super::doorbell::{self, Moving, Now};
use super::presence::{Panes, Seen};
use super::prompt::{self, AtPrompt};
use crate::session::Shared;
use crate::writer::Input;

/// A rename typed and not yet seen taken, by pane.
pub(crate) type Typing = HashMap<String, Typed>;

#[derive(Debug)]
pub(crate) struct Typed {
    name: String,
    line: String,
    /// When it was typed, or Return last pressed for it.
    at: Instant,
    presses: u8,
}

/// Types each pane's name still wanted into its agent's prompt where the pane allows, and looks
/// again at each typed earlier, leaving alone the panes `busy` names, where something else is
/// handing the agent a line. Says through `next` when it wants to look again. Called with no lock
/// held.
pub(crate) fn look(
    shared: &Shared,
    panes: &Panes,
    busy: &HashSet<String>,
    now: Instant,
    typing: &mut Typing,
    moving: &mut Moving,
    next: &mut Option<Instant>,
) {
    second_looks(shared, panes, now, typing, next);
    for (pane, seen) in panes.iter() {
        let Some(rename) = &seen.rename else { continue };
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
        if !seen.io.queue(Input::Ring { text: prompt::as_typed(&rename.line), enter: true }) {
            given_up(shared, pane, &rename.name, "its pane would not take what was typed");
            continue;
        }
        log::info("session.name.typed", fields! { "pane" => pane, "agent" => seen.agent });
        let at = Instant::now();
        typing.insert(
            pane.clone(),
            Typed { name: rename.name.clone(), line: rename.line.clone(), at, presses: 0 },
        );
        doorbell::sooner(next, at + doorbell::SETTLE);
    }
}

/// Looks again at each rename typed, once the screen has had time to show what came of it.
fn second_looks(
    shared: &Shared,
    panes: &Panes,
    now: Instant,
    typing: &mut Typing,
    next: &mut Option<Instant>,
) {
    let mut taken = Vec::new();
    typing.retain(|pane, typed| {
        // Gone with its agent, or nothing more to type: the harness already said the name. A pane
        // renamed again still has this one seen through first, since it may sit unsent in the
        // prompt, where the next would wait on it for good.
        let Some(seen) = panes.get(pane) else { return false };
        if seen.rename.is_none() {
            return false;
        }
        let due = typed.at + doorbell::SETTLE;
        if due > now {
            doorbell::sooner(next, due);
            return true;
        }
        match second_look(shared, seen, typed) {
            Second::Taken => {
                taken.push((pane.clone(), typed.name.clone(), None));
                false
            }
            Second::Pressed => {
                doorbell::sooner(next, typed.at + doorbell::ANSWER);
                true
            }
            Second::Ended(why) => {
                taken.push((pane.clone(), typed.name.clone(), Some(why)));
                false
            }
        }
    });
    for (pane, name, ended) in taken {
        match ended {
            None => {
                log::info("session.name.taken_by_session", fields! { "pane" => pane });
                shared.lock().session_name_typed(&pane, &name);
            }
            Some(why) => given_up(shared, &pane, &name, why),
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
        AtPrompt::Holds(_) => Second::Ended("its prompt holds more than the rename"),
        AtPrompt::Not(why) => Second::Ended(why),
    }
}

/// Gives up typing a name, which is then not typed again until the pane is renamed.
fn given_up(shared: &Shared, pane: &str, name: &str, why: &str) {
    log::warn(
        "session.name.given_up",
        fields! {
            "pane" => pane,
            "why" => why,
            "impact" => "the agent's session keeps the name it had; the pane's name is not \
                         typed into it again until the pane is renamed",
            "check" => "what the pane shows: the rename may sit in the agent's prompt beside \
                        something typed after it, or a dialog may have opened over it",
        },
    );
    shared.lock().session_name_typed(pane, name);
}
