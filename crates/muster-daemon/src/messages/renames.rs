//! A pane's name typed into its agent's session (MIP-5, section 10): the line the agent's
//! manifest gives in `[session] rename`, typed as [`super::typist`] types any line.
//!
//! A rename is taken once the prompt is empty again, or once the harness reports the name, which
//! lets go of it unseen. One given up is not typed again until the pane is renamed.

use std::collections::HashSet;
use std::time::Instant;

use muster_core::diagnostics::log;
use muster_core::fields;

use super::doorbell::Moving;
use super::presence::{Panes, Seen};
use super::typist::{self, Line};
use crate::session::Shared;

/// A rename typed and not yet seen taken, by pane, with the name it gives.
pub(crate) type Typing = typist::Typing<String>;

struct Rename;

impl Line for Rename {
    type What = String;
    const NOUN: &'static str = "rename";
    const TYPED: &'static str = "session.name.typed";
    const TAKEN_AT_WORK: bool = false;

    fn wanted(seen: &Seen) -> Option<(String, &str)> {
        seen.rename.as_ref().map(|rename| (rename.name.clone(), rename.line.as_str()))
    }

    /// Nothing more to type once the harness has said the name.
    fn still_wanted(seen: &Seen) -> bool {
        seen.rename.is_some()
    }

    fn taken(shared: &Shared, pane: &str, name: &String) {
        log::info("session.name.taken_by_session", fields! { "pane" => pane });
        shared.lock().session_name_typed(pane, name);
    }

    /// Gives up typing a name, which is then not typed again until the pane is renamed.
    fn given_up(shared: &Shared, pane: &str, name: &String, why: &str) {
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
}

/// Types each pane's name still wanted into its agent's prompt, and looks again at each typed
/// earlier ([`typist::look`]).
pub(crate) fn look(
    shared: &Shared,
    panes: &Panes,
    busy: &HashSet<String>,
    now: Instant,
    typing: &mut Typing,
    moving: &mut Moving,
    next: &mut Option<Instant>,
) {
    typist::look::<Rename>(shared, panes, busy, now, typing, moving, next);
}
