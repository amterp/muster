//! The one thread that notices a pane nobody ever dialed, and one that has stopped painting.
//!
//! `muster_core::typeable` decides which panes have waited too long for a bridge and
//! `muster_core::painting` which have been asked for something and answered nothing; this holds
//! the clock and the thread that asks them both, and turns the answers into problems the roster
//! draws and bridges the shell builds surfaces for. Split that way because the deadline is the
//! only part of this a case cannot reach: a fold over (pane, when it started, what time it is
//! now) is testable, and a thread parked on a condvar is not.
//!
//! One thread for the two watches, because they are the same shape of question about the same
//! panes and a second one parked on a second condvar would be two ways to answer it. It sleeps
//! until the earlier of what the two are owed.
//!
//! The asking is here rather than beside the replacement policy because this is the only thing
//! in the process that knows a bridge never arrived. Everything else that asks for one is
//! driven by a bridge *ending*, and a replacement decided on and never started has no exit -
//! so before this, a pane in that state had no bridge for the life of the app process with its
//! agent still running behind it (kan a_2KIPfvt7L).
//!
//! **Only this module raises or clears a pane's problem.** The call sites in `session.rs` do
//! nothing but record into `WAITING` or `PAINTING` and knock, and that is a lock rule rather
//! than a style preference. `raise_problem` takes `PROBLEMS` and then `SESSION`, while
//! `open_channel` runs holding `SESSION` and wants `WAITING` - so a call site that raised while
//! holding either watch would be the other half of an AB/BA deadlock against the thread that
//! accepted a connection. One writer removes the ordering question instead of documenting an
//! answer to it.
//!
//! Both watches are leaves, on the same terms and for the same reason: either may be taken
//! while `SESSION` is held, and nothing may take `SESSION` while holding one. They are also
//! never held at once - the loop below reconciles one, drops it, then the other - so no order
//! between them exists to get wrong.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use muster_core::PaneKey;
use muster_core::composition::DaemonId;
use muster_core::diagnostics::{clock, log, poison};
use muster_core::fields;
use muster_core::painting::Painting;
use muster_core::problems::Severity;
use muster_core::respawn::Ended;
use muster_core::typeable::Waiting;

use crate::session;

/// How long a pane may wait for a bridge before saying so, in milliseconds.
///
/// `tools/smoke-launch.py` waits 2.0s after `app.ready` for a healthy bridge to "start, dial
/// back and paint", so five seconds is about two and a half times the budget a working launch
/// is already known to fit inside - long enough that a machine under load does not get
/// accused, short enough that nobody has typed into a deaf pane and drawn their own
/// conclusions first.
const DEADLINE_MS: u64 = 5_000;

/// The deadline this run is using, in nanoseconds, read from the environment once.
///
/// `MUSTER_TYPEABLE_DEADLINE_MS` overrides it, and `0` switches the watch off. It is here
/// rather than in `config.toml` because its reason to exist is the suite: the seam's own tests
/// attach a real daemon and never start a bridge, so every pane in them is genuinely
/// untypeable, and proving that end to end should not cost five seconds of gate. A run that
/// has no bridges to wait for can say so, which is the same knob from the other side.
static DEADLINE: LazyLock<u64> = LazyLock::new(|| {
    let Ok(spelled) = std::env::var("MUSTER_TYPEABLE_DEADLINE_MS") else {
        return DEADLINE_MS * 1_000_000;
    };
    match spelled.trim().parse::<u64>() {
        Ok(millis) => {
            log::info("typeable.deadline.overridden", fields! { "millis" => millis.to_string() });
            millis.saturating_mul(1_000_000)
        }
        Err(error) => {
            log::warn(
                "typeable.deadline.unreadable",
                fields! {
                    "value" => spelled,
                    "detail" => error.to_string(),
                    "impact" => format!(
                        "MUSTER_TYPEABLE_DEADLINE_MS is not a whole number of milliseconds, so \
                         the default of {DEADLINE_MS}ms is in force and a pane that never \
                         becomes typeable is reported after that instead"
                    ),
                    "check" => "write a count of milliseconds, or 0 to stop watching for it",
                },
            );
            DEADLINE_MS * 1_000_000
        }
    }
});

/// The panes being waited on, and the door the thread is knocked on.
///
/// A leaf lock: nothing is called while it is held. See the module comment for why that
/// matters rather than merely being tidy.
static WAITING: Mutex<Waiting> = Mutex::new(Waiting::new());

/// The panes that owe a frame, on the same terms as `WAITING` beside it.
static PAINTING: Mutex<Painting> = Mutex::new(Painting::new());

static KNOCK: Condvar = Condvar::new();
static WATCHING: AtomicBool = AtomicBool::new(false);

/// How long a pane may owe a frame before its silence is worth reporting, in milliseconds.
///
/// Ten seconds, and the number is chosen by the false alarm rather than by the healthy case. A
/// working pane answers a keystroke in milliseconds, so any deadline at all catches a freeze;
/// what sets the floor is the longest an honest pane can paint nothing after input, which is a
/// program that turned echo off and is waiting for a password. Ten seconds is past any password
/// anybody types, and still two orders of magnitude better than the sixteen minutes of frozen
/// pane this was written for.
const PAINTING_DEADLINE_MS: u64 = 10_000;

/// The deadline this run watches painting on, in nanoseconds, read from the environment once.
///
/// `MUSTER_PAINTING_DEADLINE_MS` overrides it and `0` switches the watch off, on the same terms
/// as the two knobs beside it: a suite proving this end to end should not cost ten seconds of
/// gate, and a run with no frames to wait for should be able to say so.
static PAINTING_DEADLINE: LazyLock<u64> = LazyLock::new(|| {
    let Ok(spelled) = std::env::var("MUSTER_PAINTING_DEADLINE_MS") else {
        return PAINTING_DEADLINE_MS * 1_000_000;
    };
    match spelled.trim().parse::<u64>() {
        Ok(millis) => {
            log::info("painting.deadline.overridden", fields! { "millis" => millis.to_string() });
            millis.saturating_mul(1_000_000)
        }
        Err(error) => {
            log::warn(
                "painting.deadline.unreadable",
                fields! {
                    "value" => spelled,
                    "detail" => error.to_string(),
                    "impact" => format!(
                        "MUSTER_PAINTING_DEADLINE_MS is not a whole number of milliseconds, so \
                         the default of {PAINTING_DEADLINE_MS}ms is in force and a pane that \
                         stops painting is reported after that instead"
                    ),
                    "check" => "write a count of milliseconds, or 0 to stop watching for it",
                },
            );
            PAINTING_DEADLINE_MS * 1_000_000
        }
    }
});

/// A pane's socket is bound, so its bridge is expected from now.
///
/// `backend` is what the daemon calls this pane, taken here because this is where the seam has
/// it. The one remedy that is about a herdr process rather than about Muster is matched against
/// that client's command line, and a pattern built from the name in this window matches nothing.
pub(crate) fn opened(pane: PaneKey, backend: String) {
    poison::lock(&WAITING, "typeable").opened(pane, clock::monotonic_now(), backend);
    start();
}

/// A pane's bridge ended, so the wait restarts - and this time it knows why.
///
/// The whole difference between "look in the run log" and a sentence naming what happened and
/// what releases it. A pane that stays dark after this says so on its own row five seconds
/// later, which is the one surface a person actually sees.
pub(crate) fn ended(pane: PaneKey, ended: Ended) {
    poison::lock(&WAITING, "typeable").ended(pane, clock::monotonic_now(), ended);
    start();
}

/// A bridge dialed in.
pub(crate) fn typeable(pane: &PaneKey) {
    poison::lock(&WAITING, "typeable").typeable(pane);
    KNOCK.notify_all();
}

/// Something reached this pane, so it owes a frame.
///
/// On the input path, so it does as little as it can: a pane that already owed a frame is not
/// news, and knocking on every keystroke would wake the thread for a reading it has already
/// taken.
pub(crate) fn typed(pane: &PaneKey) {
    let news = poison::lock(&PAINTING, "painting").typed(pane, clock::monotonic_now());
    if news {
        start();
    }
}

/// The pane painted, so it owes nothing.
///
/// Arrives up to four times a second per pane while one is painting, and the overwhelmingly
/// common case is a pane that owed nothing - so the same rule as above, from the other side:
/// nothing that changes what the clock owes, nothing to wake anybody for.
pub(crate) fn painted(pane: &PaneKey) {
    let news = poison::lock(&PAINTING, "painting").painted(pane);
    if news {
        KNOCK.notify_all();
    }
}

/// Whether something else has already said why this pane is silent.
///
/// The grid ceiling is the one that does: a pane too big to draw has a remedy in its own
/// sentence, and a second row saying only that it stopped painting would send its reader away
/// from the answer they already had.
pub(crate) fn explained(pane: &PaneKey, explained: bool) {
    poison::lock(&PAINTING, "painting").explained(pane, explained);
    KNOCK.notify_all();
}

/// Whether this daemon is answering, so that its panes are not blamed for a machine's silence.
pub(crate) fn daemon_away(daemon: &DaemonId, away: bool) {
    poison::lock(&PAINTING, "painting").daemon_away(daemon, away);
    KNOCK.notify_all();
}

/// The pane is gone, so nothing is owed about it.
pub(crate) fn closed(pane: &PaneKey) {
    poison::lock(&WAITING, "typeable").closed(pane);
    poison::lock(&PAINTING, "painting").closed(pane);
    KNOCK.notify_all();
}

/// Which panes the window is drawing, straight from the view it just published.
///
/// Nothing is started here. A pane the window began drawing waits a full deadline from now,
/// so there is never anything to say at this moment - and a run that has opened no pane
/// should not gain a thread for having published a view of nothing.
pub(crate) fn showing(visible: BTreeSet<PaneKey>) {
    poison::lock(&PAINTING, "painting").showing(visible.clone());
    poison::lock(&WAITING, "typeable").showing(visible, clock::monotonic_now());
    KNOCK.notify_all();
}

/// Forgets every pane being waited on, for a process starting over.
///
/// The thread stays. It is parked on the condvar with nothing owed, which is what it does
/// between panes anyway, and stopping it would mean a way to start a second one - which is a
/// mechanism nothing in a shipped window would ever use.
pub(crate) fn forget_everything() {
    *poison::lock(&WAITING, "typeable") = Waiting::new();
    *poison::lock(&PAINTING, "painting") = Painting::new();
    KNOCK.notify_all();
}

/// Starts the thread that watches the clock, once per process.
///
/// Lazily, because a run that never opens a pane should not carry a thread, and parked on a
/// condvar rather than ticking, because a window whose panes are all typeable has nothing for
/// it to do and an idle window should cost no wakeups at all.
fn start() {
    if *DEADLINE == 0 && *PAINTING_DEADLINE == 0 {
        return;
    }
    if WATCHING.swap(true, Ordering::AcqRel) {
        KNOCK.notify_all();
        return;
    }
    std::thread::spawn(watch);
}

fn watch() {
    let deadline = *DEADLINE;
    let painting_deadline = *PAINTING_DEADLINE;
    loop {
        let reported = {
            let mut waiting = poison::lock(&WAITING, "typeable");
            waiting.reconcile(clock::monotonic_now(), deadline)
        };
        for (key, detail) in reported.raise {
            session::raise_problem(&key, Severity::Error, &detail);
        }
        for key in reported.clear {
            session::clear_problem(&key);
        }
        // Outside the lock, like the two above and for the same reason: this reaches `SESSION`
        // and publishes, and publishing comes back through `showing` for `WAITING`.
        for pane in reported.stalled {
            session::bridge_stalled(&pane, deadline);
        }

        // The other watch, from a lock taken and dropped on its own. A warning rather than an
        // error: severity here decides whether a closed sidebar is forced open, and this is a
        // condition that may clear by itself and can be raised about a pane that turns out to
        // have been fine - where a pane too big for a frame knows exactly what is wrong with it
        // and will not fix itself.
        let painted = {
            let mut painting = poison::lock(&PAINTING, "painting");
            painting.reconcile(clock::monotonic_now(), painting_deadline)
        };
        for (key, detail) in painted.raise {
            session::raise_problem(&key, Severity::Warning, &detail);
        }
        for key in painted.clear {
            session::clear_problem(&key);
        }

        // Asked again under the guard this waits on, rather than reused from above. A pane
        // opened while those problems were being published would otherwise be slept through -
        // its knock lands while nobody is waiting, and on a quiet window nothing else arrives
        // to wake this up. Holding the lock across the wait is what makes a later knock
        // reliable.
        //
        // The painting watch is asked first and its answer carried in, because only one lock
        // can be held across the wait and the other has to be given up before this one is
        // taken. What that costs is a pane that fell silent in the moment between the two
        // readings waiting one wake longer to be reported, which is a wake it would have spent
        // asleep anyway.
        let owed = poison::lock(&PAINTING, "painting")
            .next_wake(clock::monotonic_now(), painting_deadline);
        let waiting = poison::lock(&WAITING, "typeable");
        let sleep = match (waiting.next_wake(clock::monotonic_now(), deadline), owed) {
            (Some(waited), Some(owed)) => Some(waited.min(owed)),
            (waited, owed) => waited.or(owed),
        };
        match sleep {
            Some(nanos) => drop(KNOCK.wait_timeout(waiting, Duration::from_nanos(nanos))),
            // Nothing more happens on the clock alone: every pane still waiting is already
            // reported, and everything that can change that knocks.
            None => drop(KNOCK.wait(waiting)),
        }
    }
}
