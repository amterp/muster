//! What is wrong, which every window's roster lists, and keeping each window's roster open while
//! an error stands.
//!
//! Anything in the core that finds something wrong raises it here by a key and clears it by the
//! same key once it stops being true: a config that does not read, a daemon that cannot be
//! reached, a pane no bridge has dialed. The list is process-wide rather than part of a
//! [`super::Session`], because nothing in it is written down and every window shows the same
//! one. What it does to a window is the session's to apply - opening a roster an error needs to
//! be seen in, and giving it back once the error clears - so that half reaches into the session.

use std::collections::BTreeSet;
use std::sync::Mutex;

use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_core::problems::{Problem, Problems, Remedy, Severity};

use super::{SESSION, WindowId, set_sidebar};
use crate::ffi;
use crate::proto::{
    Event, Problem as ProblemMessage, ProblemsChanged, ReattachPane, Request, event, problem,
    request,
};

/// What is wrong, which every window's roster lists.
///
/// Beside the settings rather than in `SESSION` because a problem is not part of an
/// arrangement: nothing here is written to `window.toml` and nothing survives a launch. A
/// config still broken on the next launch is raised again by reading it again, which is the
/// only answer that cannot go stale.
static PROBLEMS: Mutex<Option<Problems>> = Mutex::new(None);

/// Records that something is wrong, and makes sure somebody can see it.
///
/// Everything a caller has to remember is in here on purpose. Publishing only on a real
/// change is what stops a file saved repeatedly with one typo from reopening a roster
/// somebody keeps closing, and doing the roster and the event together is what stops one
/// from being forgotten at a new call site.
pub(crate) fn raise_problem(key: &str, severity: Severity, detail: &str) {
    raise_problem_with_remedy(key, severity, detail, None);
}

/// [`raise_problem`], with something to offer beside the sentence as one click.
pub(crate) fn raise_problem_with_remedy(
    key: &str,
    severity: Severity,
    detail: &str,
    remedy: Option<&Remedy>,
) {
    let changed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        held.get_or_insert_with(Problems::new).raise(key, severity, detail, remedy)
    };
    if !changed {
        return;
    }
    // The sentence a person actually read, which the run log otherwise lacked: the conditions
    // that raise a problem mostly log themselves, and the watches on panes log nothing, so the
    // one line an incident turned on had to be inferred from the events around it (kan
    // a_2LMpvavhA). Info rather than a level taken from the severity, because the condition's
    // own record carries the level where there is one.
    log::info(
        "problem.raised",
        fields! {
            "key" => key,
            "severity" => severity.as_str(),
            "detail" => detail,
            "remedy" => remedy.map(Remedy::title).unwrap_or_default(),
        },
    );
    reconcile_sidebars_with_problems();
    announce_problems();
}

/// Records that something is no longer wrong, and why.
///
/// Called from every success path, including the ones where nothing was ever wrong, so the
/// common case is a call that changes nothing and says nothing.
///
/// `why` goes to the run log, because a problem going away is not always the thing it was about
/// being fixed. A painting warning cleared by the pane painting and one cleared because nobody
/// could see the pane any more used to be the same record, and only one of them meant the pane
/// was working (kan a_2LWqtPd8E).
pub(crate) fn clear_problem(key: &str, why: &str) {
    let changed = {
        let mut held = poison::lock(&PROBLEMS, "problems");
        held.get_or_insert_with(Problems::new).clear(key)
    };
    if !changed {
        return;
    }
    log::info("problem.cleared", fields! { "key" => key, "why" => why });
    reconcile_sidebars_with_problems();
    announce_problems();
}

/// Everything wrong, worst first.
pub(crate) fn problems() -> Vec<Problem> {
    poison::lock(&PROBLEMS, "problems").as_ref().map(Problems::outstanding).unwrap_or_default()
}

/// Makes the roster's visibility agree with whether an error is outstanding.
///
/// Derived rather than decided at the moment a problem arrives, because the two inputs land in
/// an order nothing guarantees. A config refused during `Startup` raises its problem before
/// `open()` has restored whether the roster was open at all, so a version of this that checked
/// once at raise time checked a default that was about to be replaced - and opened nothing, for
/// a window that came back with the roster put away and a broken config. Reconciling from both
/// sides makes the order stop mattering.
///
/// Only errors open a roster. A warning in a list somebody will look at eventually is fine, and
/// reflowing every pane to mention a stale daemon would be worse than staying quiet. Reflowing
/// is the real cost here and it is why the line is drawn: somebody typing when their config
/// breaks gets their panes resized underneath them, which is accepted, because the alternative
/// is the silence that cost an evening.
///
/// And only an error the window has not already shown (`Window::shown_errors`). Somebody who put
/// the roster away while an error stood has seen it, and opening it again each time that error's
/// sentence changed, or another problem came or went, took the roster back from them.
///
/// Answers whether it moved the roster, so a caller mid-announcement does not say it twice.
///
/// Every window lists the same problems, so each one's roster is reconciled on its own terms:
/// borrowed where it was closed, and given back only where it was borrowed.
pub(super) fn reconcile_sidebar_with_problems(window: WindowId) -> bool {
    let errors: BTreeSet<String> = poison::lock(&PROBLEMS, "problems")
        .as_ref()
        .map(|problems| {
            problems
                .outstanding()
                .into_iter()
                .filter(|problem| problem.severity == Severity::Error)
                .map(|problem| problem.key)
                .collect()
        })
        .unwrap_or_default();
    let (shown, name, unseen) = {
        let mut session = poison::lock(&SESSION, "session");
        let held = &mut session.windows[window];
        let unseen = errors.difference(&held.shown_errors).next().is_some();
        held.shown_errors.retain(|key| errors.contains(key));
        if held.presentation.sidebar || unseen {
            held.shown_errors.clone_from(&errors);
        }
        (held.presentation.sidebar, held.name.to_string(), unseen)
    };

    if !errors.is_empty() {
        if shown || !unseen {
            return false;
        }
        poison::lock(&SESSION, "session").windows[window].opened_sidebar = true;
        log::info(
            "problems.sidebar.opened",
            fields! {
                "window" => name,
                "impact" => "the roster was closed and an error would have had nowhere to \
                             appear, so Muster opened it",
                "check" => "it closes again on its own when the last error clears, unless you \
                            open or close it yourself first",
            },
        );
        set_sidebar(window, true);
        return true;
    }

    // Borrowed, so give it back. Only when Muster was the one who opened it: a roster somebody
    // opened themselves is theirs, and closing it because a problem happened to clear would be
    // Muster tidying away a window it does not own.
    let borrowed =
        std::mem::take(&mut poison::lock(&SESSION, "session").windows[window].opened_sidebar);
    if borrowed && shown {
        log::info(
            "problems.sidebar.closed",
            fields! {
                "window" => name,
                "impact" => "the last error cleared, so the roster Muster opened to show it \
                             has been put back the way it was found",
            },
        );
        set_sidebar(window, false);
        return true;
    }
    false
}

/// [`reconcile_sidebar_with_problems`] for every open window, after a problem came or went.
fn reconcile_sidebars_with_problems() {
    let windows = poison::lock(&SESSION, "session").windows.opened();
    for window in windows {
        reconcile_sidebar_with_problems(window);
    }
}

fn announce_problems() {
    let problems = problems();
    ffi::emit(&Event::new(event::Payload::ProblemsChanged(ProblemsChanged {
        problems: problems
            .into_iter()
            .map(|problem| ProblemMessage {
                key: problem.key,
                severity: problem.severity.as_str().to_string(),
                detail: problem.detail,
                remedy: problem.remedy.as_ref().map(remedy_message),
            })
            .collect(),
    })));
}

/// A remedy as the shell sends it back: the request whole, with its pane named. No window is
/// named, because the core finds the window from the pane (`mip/0006-one-process.md`).
fn remedy_message(remedy: &Remedy) -> problem::Remedy {
    let payload = match remedy {
        Remedy::Reattach(pane) => request::Payload::ReattachPane(ReattachPane {
            daemon_id: pane.daemon.to_string(),
            pane_id: pane.pane.to_string(),
        }),
    };
    problem::Remedy { title: remedy.title().to_string(), request: Some(Request::new(payload)) }
}

/// Clears the list, for a [`super::reset`].
pub(super) fn forget() {
    *poison::lock(&PROBLEMS, "problems") = None;
}
