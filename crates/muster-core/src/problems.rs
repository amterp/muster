//! What is wrong with this window, in the words of whatever found it.
//!
//! Muster has a lot to say about its own failures and, until this existed, said all of it
//! into a log file. That serves the run log's purpose, which is a bug report an agent can
//! read, and does nothing for the person at the keyboard: a config refused at 18:55 left a
//! window running default keybindings all evening, and the only thing that ever mentioned it
//! was a JSON line nobody had reason to open. `README.md` already promised better ("a file
//! that will not parse changes nothing at all and says so"), so this is the half that was
//! missing rather than a new promise.
//!
//! A problem is a *condition*, not a message. It is raised while something is true and
//! cleared when it stops being true, so nothing here is ever "delivered" or "read". That
//! single decision answers most of the questions a notification system otherwise raises:
//! there is no unread count, no way to acknowledge something that is still broken, and no
//! way for the list to disagree with reality. It also means fixing a config and watching
//! the problem disappear is the confirmation that the fix worked - which is the other half
//! of the bug above, since nothing told anybody when the file started parsing again either.
//!
//! Keyed, because the same condition found twice is one problem. A watcher that reports
//! every save of a file somebody is still editing would otherwise stack ten copies of one
//! typo, and each one would be a fresh reason to reopen a sidebar they just closed.
//!
//! Pure - no clock, no window, no file. Raising and clearing is a fold over conditions, and
//! the interesting rules (a repeated raise changes nothing, an error outranks a warning) are
//! exactly what a recorded case can drive.

use std::collections::BTreeMap;

/// How much a problem asks of the person reading it.
///
/// The split is what somebody has to *do*, not how bad it feels. That is what makes the
/// field worth having: an error is what opens a sidebar that was closed, and a warning is
/// content to be found when somebody next looks. Rank them wrong and either the window
/// nags, or the thing that mattered waits behind the thing that did not.
///
/// Two levels, because a third has nothing to do yet. An informational problem is arguably
/// not a problem, and the run log is already the place for things nobody must act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Muster cannot do what was asked and will not recover on its own - somebody has to
    /// change something. A refused config file is the worked example: every setting in it is
    /// inert, and no amount of waiting applies it.
    Error,

    /// Working, but degraded or not what you probably expect, and it may clear by itself. A
    /// daemon gone stale is this: Muster is coping and the connection may come back.
    Warning,
}

impl Severity {
    /// The word for the wire and the log. Lowercase because every other enum crossing the
    /// seam is (`agent_state`, `option_as_alt`), and one that shouted would be the odd one.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One thing that is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Names the condition rather than the occasion, so raising it again replaces rather
    /// than repeats. `config.refused` is one condition however many times a file is saved.
    pub key: String,

    pub severity: Severity,

    /// What to tell the person, whole, in the words of whoever found it. Muster's refusals
    /// are already written to be read by whoever caused them - they name the file, the
    /// value, what stopped working and what to type instead - so passing one through
    /// untouched beats anything this module could compose about it.
    pub detail: String,
}

/// Everything wrong with this window right now.
#[derive(Debug, Default)]
pub struct Problems {
    /// Keyed by condition. A `BTreeMap` rather than a `Vec` so that raising the same
    /// condition twice cannot produce two entries, and so the order two runs report is the
    /// same order.
    raised: BTreeMap<String, (Severity, String)>,
}

impl Problems {
    pub fn new() -> Problems {
        Problems::default()
    }

    /// Records that something is wrong, and says whether that was news.
    ///
    /// The answer is what stops a watcher from fighting the person using it. Saving a file
    /// that is still broken in the same way is not a new problem, so it must not republish
    /// and must not reopen a sidebar somebody just closed. Only a genuine change - a first
    /// raise, a different message, a different severity - is worth anybody's attention.
    pub fn raise(&mut self, key: &str, severity: Severity, detail: &str) -> bool {
        let fresh = (severity, detail.to_string());
        match self.raised.get(key) {
            Some(held) if *held == fresh => false,
            _ => {
                self.raised.insert(key.to_string(), fresh);
                true
            }
        }
    }

    /// Records that something is no longer wrong, and says whether that was news.
    ///
    /// Clearing a condition nobody raised is not an error and not a no-op worth reporting:
    /// every success path clears the problem its failure path raises, so the common call is
    /// "clear the thing that was already fine".
    pub fn clear(&mut self, key: &str) -> bool {
        self.raised.remove(key).is_some()
    }

    /// Everything outstanding, worst first.
    ///
    /// Errors before warnings, and within a severity by key. Sorted here rather than in
    /// whatever draws it, because two surfaces render this - a list and a window title that
    /// has room for one - and they must not disagree about which problem is the important
    /// one.
    pub fn outstanding(&self) -> Vec<Problem> {
        let mut problems: Vec<Problem> = self
            .raised
            .iter()
            .map(|(key, (severity, detail))| Problem {
                key: key.clone(),
                severity: *severity,
                detail: detail.clone(),
            })
            .collect();
        problems.sort_by(|left, right| {
            left.severity.cmp(&right.severity).then_with(|| left.key.cmp(&right.key))
        });
        problems
    }

    /// Whether anything outstanding is an error.
    ///
    /// The one question a caller asks that is not "what have you got": an error is worth
    /// interrupting somebody for and a warning is not, so this is what decides whether a
    /// closed sidebar gets opened.
    pub fn has_error(&self) -> bool {
        self.raised.values().any(|(severity, _)| *severity == Severity::Error)
    }

    pub fn is_empty(&self) -> bool {
        self.raised.is_empty()
    }
}
