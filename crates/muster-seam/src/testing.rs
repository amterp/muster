//! One test's turn with the seam.
//!
//! There is one window per process, so the seam holds its session in a global - which is the
//! honest expression of the arrangement and was also, for a while, a rule about tests: a test
//! binary is a process too, so a second test in one could not have a session of its own. Every
//! file under `tests/` held exactly one `#[test]`, and adding a gesture cost a new file and a
//! new daemon.
//!
//! What that actually cost was bisectability rather than coverage. A file that could hold one
//! test held one *scenario* instead - `attach.rs` chained nine through helper functions - and a
//! chained scenario stops at the first failure, so the later behaviour never runs and one red
//! run cannot say whether that broke too.
//!
//! This is the way out: a lock so the tests in a binary take their turns, and a reset so each
//! turn starts where a fresh process would.
//!
//! Not behind a feature flag. A flag would have to be turned on by the test targets of this
//! crate, which is every caller there will ever be, and the shipped dylib exports what
//! `include/muster.h` declares rather than everything this crate happens to make public.

use std::sync::{Mutex, MutexGuard};

use crate::session;

/// Which test is using the seam. Held for the length of one, so the tests in a binary run one
/// at a time rather than racing each other through one session.
static TURN: Mutex<()> = Mutex::new(());

/// A session nobody else has touched, for the length of this test.
///
/// Take it as the first line of a `#[test]` and keep the guard alive for the whole of it:
///
/// ```ignore
/// #[test]
/// fn a_pane_can_be_typed_into() {
///     let _turn = muster::testing::fresh_session();
///     let daemon = Daemon::start();
///     ...
/// }
/// ```
///
/// The reset happens on the way in rather than on the way out, which is what makes a test that
/// panics cost only itself: whatever it left behind is cleared by whoever goes next, and there
/// is no teardown for a panic to skip. A poisoned lock is recovered for the same reason the
/// rest of the seam recovers its own - the next test should fail on its own assertions, not on
/// the last one's ghost.
///
/// The environment is not reset, and cannot be: it is read once per process by things like the
/// typeable deadline, and it belongs to the binary rather than to a test. A file a test writes
/// is its own business; every helper here already puts one under the daemon's scratch root.
#[must_use]
pub fn fresh_session() -> Turn {
    let turn = TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    session::reset();
    Turn { _turn: turn }
}

/// One test's turn, given up when it is dropped.
#[derive(Debug)]
pub struct Turn {
    _turn: MutexGuard<'static, ()>,
}

impl Turn {
    /// Quits this window and opens another one, inside the same test.
    ///
    /// The reset `fresh_session` does, without taking the turn again - the lock is not
    /// reentrant, so a test that called `fresh_session` twice would hang rather than
    /// relaunch.
    ///
    /// What it buys is the one behaviour a single launch structurally cannot show: what a
    /// window does with the arrangement the launch before it wrote down. A file on disk is
    /// all that carries between the two, which is exactly what carries between two
    /// processes, so a test that relaunches is testing the same seam a second process would
    /// come through.
    ///
    /// The event callback goes with everything else, so a caller that was watching events
    /// has to set it again before driving the second launch.
    pub fn relaunch(&self) {
        session::reset();
    }
}
