//! One answer to "a thread panicked while holding this lock", instead of ninety.
//!
//! A poisoned lock is not itself a bug - it is the *second* symptom of one, and what a
//! process does about it decides whether the first bug costs a keystroke or the window.
//! Muster had two policies and had chosen neither on purpose:
//!
//! - `.expect(..)` on the lock, which turns one panic into every later request panicking.
//!   The seam catches those before they cross the C ABI and tells the user that "later
//!   requests are unaffected" - which stops being true the moment the panic happened under
//!   a lock, because the next request panics in the same place. The window then renders
//!   pane output forever, from the data plane, while ignoring every key and never updating
//!   its view.
//! - `.lock().ok()?`, which drops the value and carries on. A poisoned mirror makes its
//!   daemon vanish out of the view and the roster with nothing said about it - the panes
//!   are still there, still running, and the window has stopped mentioning them.
//!
//! Both fail the same standard: a run has to explain itself, and a window that ignores the
//! keyboard has to be able to say why (`README.md`, every run explains itself;
//! `architecture.md`, composition is resolved against the mirror).
//!
//! **So the policy is to recover the value and say so.** What sits behind these locks is
//! either a plain setting written whole, or a mirror of daemon truth - and a mirror is "a
//! derived, disposable cache ... rebuilt after any gap, never patched across one"
//! (`architecture.md`, ownership of truth), which is exactly a thing it is safe to pick up
//! and re-derive. Recovery is therefore not optimism about the data; it is the one option
//! that leaves the process able to report the panic that started this.
//!
//! The poison flag is cleared as it is recovered, so the warning marks the panic rather
//! than repeating on every lock taken afterwards. That matters more than it sounds: the
//! session lock is taken on every request, so a warning per acquisition would bury the one
//! record naming the original panic under thousands of copies of itself.

use std::sync::{LockResult, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::diagnostics::log;
use crate::fields;

/// Locks a mutex, recovering it if a panicking holder poisoned it.
///
/// `what` names the thing being locked, in the same dotted style as a log event -
/// `session`, `mirror`, `bindings` - because it is what a reader greps for.
pub fn lock<'a, T>(mutex: &'a Mutex<T>, what: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            mutex.clear_poison();
            report(what);
            poisoned.into_inner()
        }
    }
}

/// Takes a read lock, recovering it if a panicking holder poisoned it.
pub fn read<'a, T>(lock: &'a RwLock<T>, what: &str) -> RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            report(what);
            poisoned.into_inner()
        }
    }
}

/// Takes a write lock, recovering it if a panicking holder poisoned it.
pub fn write<'a, T>(lock: &'a RwLock<T>, what: &str) -> RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            report(what);
            poisoned.into_inner()
        }
    }
}

/// Recovers a lock whose guard the caller already has a `LockResult` for.
///
/// For the call sites that lock through something other than a plain `&Mutex` - a field
/// reached through a temporary, most often - where the borrow checker will not let the
/// mutex be named twice. Cannot clear the poison flag, so it warns every time; prefer
/// [`lock`] wherever the mutex can be named.
pub fn recover<T>(result: LockResult<T>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(poisoned) => {
            report(what);
            poisoned.into_inner()
        }
    }
}

fn report(what: &str) {
    log::warn(
        "lock.poisoned",
        fields! {
            "lock" => what,
            "impact" => "a thread panicked while holding this lock, so whatever it was \
                         part-way through writing is left as it was. The value has been \
                         recovered and the process carries on, which means the state \
                         behind this lock may disagree with the daemon until the next \
                         snapshot puts it right",
            "check" => "the panic itself is the bug worth chasing, and it was reported \
                        just before this record - look above for a backtrace or for \
                        `core.panicked`. A poisoned lock is never the first thing to go \
                        wrong",
        },
    );
}
