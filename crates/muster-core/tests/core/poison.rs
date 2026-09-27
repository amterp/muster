//! What a lock does after a thread panics while holding it.
//!
//! Native rather than a conformance case, on the line `docs/testing.md` draws: this is a
//! property of the process rather than a behavior in Muster's vocabulary, and a second
//! implementation in another language would answer it with its own machinery.
//!
//! Worth pinning because the failure it prevents is invisible from the outside. A panic
//! under the session lock used to leave every later request panicking in the same place,
//! and the seam caught each one and answered null - so the window went on rendering pane
//! output from the data plane while ignoring every key, and nothing about it looked like a
//! crash.

use std::sync::{Mutex, RwLock};

use muster_core::diagnostics::poison;

/// The value survives the panic, and the lock keeps working.
#[test]
fn a_poisoned_mutex_is_recovered_with_what_was_written_before_the_panic() {
    let held = Mutex::new(vec![1, 2, 3]);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut guard = poison::lock(&held, "test");
        guard.push(4);
        panic!("a holder gave up part-way through");
    }));
    assert!(panicked.is_err(), "the test needs the panic to have actually happened");
    assert!(held.is_poisoned(), "a panic under the lock is what poisons it");

    // Recovered rather than re-panicked on, and carrying the write the panicking thread
    // had already made - which is the honest reading of "part-way through".
    assert_eq!(*poison::lock(&held, "test"), vec![1, 2, 3, 4]);
}

/// The warning marks the panic, not every acquisition after it.
///
/// The session lock is taken on every request, so a report per lock would bury the one
/// record naming the original panic under thousands of copies of itself.
#[test]
fn recovering_a_mutex_clears_the_poison_so_the_next_lock_is_ordinary() {
    let held = Mutex::new(0);

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = poison::lock(&held, "test");
        panic!("a holder gave up");
    }));
    assert!(held.is_poisoned());

    drop(poison::lock(&held, "test"));
    assert!(!held.is_poisoned(), "recovery clears the flag, so the next lock reports nothing");
}

#[test]
fn a_poisoned_rwlock_is_recovered_for_readers_and_for_writers() {
    let held = RwLock::new(String::from("before"));

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut guard = poison::write(&held, "test");
        guard.push_str("-during");
        panic!("a writer gave up part-way through");
    }));
    assert!(held.is_poisoned());

    assert_eq!(*poison::read(&held, "test"), "before-during");
    assert!(!held.is_poisoned());

    // And a write lock recovers the same way, rather than only the read path being fixed.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = poison::write(&held, "test");
        panic!("a second writer gave up");
    }));
    assert!(held.is_poisoned());
    poison::write(&held, "test").push_str("-after");
    assert_eq!(*poison::read(&held, "test"), "before-during-after");
}

/// A lock nobody panicked under is untouched, so the policy costs nothing in the ordinary
/// case.
#[test]
fn an_unpoisoned_lock_is_handed_over_unchanged() {
    let held = Mutex::new(7);
    *poison::lock(&held, "test") += 1;
    assert_eq!(*poison::lock(&held, "test"), 8);
    assert!(!held.is_poisoned());
}
