//! Stopping a thread that polls at a point where it has finished what it was doing, and
//! starting it again: a pane's reader while its terminal is handed to another daemon, and the
//! accept loop while the socket is (MIP-3 section 10).
//!
//! The thread polls [`Hold::polled`] beside its own descriptors, so a hold reaches it however
//! long its poll would otherwise sleep, and calls [`Hold::park`] each time round its loop.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use muster_core::diagnostics::poison;

/// How often a parked thread looks at whether it has been told to end.
const LOOK: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) struct Hold {
    state: Mutex<State>,
    changed: Condvar,
    /// Readable once the thread is asked to stop: the read end of a pipe.
    nudged: OwnedFd,
    nudge: OwnedFd,
}

#[derive(Debug, Default)]
struct State {
    held: bool,
    parked: bool,
    /// The thread has returned, and will never park.
    gone: bool,
}

impl Hold {
    /// A hold on a thread yet to start, which parks at once when `held`.
    pub(crate) fn new(held: bool) -> io::Result<Hold> {
        let mut ends = [-1; 2];
        // SAFETY: `ends` has room for the two descriptors pipe writes.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: pipe succeeded, so both are open descriptors this function now owns.
        let (nudged, nudge) =
            unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
        for end in [&nudged, &nudge] {
            // SAFETY: fcntl on a descriptor this function owns.
            let set = unsafe {
                libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) != -1
                    && libc::fcntl(end.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) != -1
            };
            if !set {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Hold {
            state: Mutex::new(State { held, ..State::default() }),
            changed: Condvar::new(),
            nudged,
            nudge,
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        poison::lock(&self.state, "daemon.hold")
    }

    /// What the held thread polls beside its own descriptors.
    pub(crate) fn polled(&self) -> RawFd {
        self.nudged.as_raw_fd()
    }

    /// Asks the thread to stop at its next safe point, and waits until it has, or has returned.
    /// False when it did neither `within`.
    pub(crate) fn hold(&self, within: Duration) -> bool {
        let mut state = self.state();
        state.held = true;
        // SAFETY: write of one byte from a live buffer. A full pipe has a nudge waiting already.
        unsafe {
            libc::write(self.nudge.as_raw_fd(), [0u8].as_ptr().cast(), 1);
        }
        let state = self
            .changed
            .wait_timeout_while(state, within, |state| !state.parked && !state.gone)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        state.parked || state.gone
    }

    /// Lets the thread go on.
    pub(crate) fn is_held(&self) -> bool {
        self.state().held
    }

    pub(crate) fn release(&self) {
        self.state().held = false;
        self.changed.notify_all();
    }

    /// Called by the held thread each time round its loop: waits here while it is held, or
    /// until `ended` says it has been told to end some other way.
    pub(crate) fn park(&self, ended: impl Fn() -> bool) {
        let mut drained = [0u8; 64];
        // SAFETY: reads into a live buffer of its length; the pipe is non-blocking.
        while unsafe { libc::read(self.nudged.as_raw_fd(), drained.as_mut_ptr().cast(), 64) } > 0 {}
        let mut state = self.state();
        if !state.held {
            return;
        }
        state.parked = true;
        self.changed.notify_all();
        while state.held && !ended() {
            state = self
                .changed
                .wait_timeout(state, LOOK)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        state.parked = false;
    }

    /// The held thread has returned: a hold asked for from now on is granted at once.
    pub(crate) fn gone(&self) {
        self.state().gone = true;
        self.changed.notify_all();
    }
}

/// Says a thread has returned when dropped, however it returned.
pub(crate) struct Leaving<'a>(pub(crate) &'a Hold);

impl Drop for Leaving<'_> {
    fn drop(&mut self) {
        self.0.gone();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Instant;

    use super::*;

    /// A thread that counts turns of its loop, polling the hold with a long timeout as a quiet
    /// pane's reader does.
    fn looping(
        hold: &Arc<Hold>,
        turns: &Arc<AtomicU64>,
        stop: &Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        let (hold, turns, stop) = (Arc::clone(hold), Arc::clone(turns), Arc::clone(stop));
        std::thread::spawn(move || {
            let _leaving = Leaving(&hold);
            while !stop.load(Ordering::Acquire) {
                hold.park(|| stop.load(Ordering::Acquire));
                turns.fetch_add(1, Ordering::AcqRel);
                let mut watched =
                    [libc::pollfd { fd: hold.polled(), events: libc::POLLIN, revents: 0 }];
                // SAFETY: one valid pollfd.
                unsafe { libc::poll(watched.as_mut_ptr(), 1, 10_000) };
            }
        })
    }

    #[test]
    fn a_hold_reaches_a_thread_asleep_in_its_poll_and_parks_it_until_released() {
        let hold = Arc::new(Hold::new(false).unwrap());
        let (turns, stop) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
        let thread = looping(&hold, &turns, &stop);
        while turns.load(Ordering::Acquire) == 0 {
            std::thread::yield_now();
        }
        let asked = Instant::now();
        assert!(hold.hold(Duration::from_secs(5)));
        assert!(asked.elapsed() < Duration::from_secs(5), "the poll's own timeout is 10 s");
        let parked_at = turns.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(turns.load(Ordering::Acquire), parked_at, "a parked thread does nothing");
        hold.release();
        while turns.load(Ordering::Acquire) == parked_at {
            std::thread::yield_now();
        }
        stop.store(true, Ordering::Release);
        hold.hold(Duration::from_secs(5));
        thread.join().unwrap();
    }

    #[test]
    fn a_thread_held_from_the_start_parks_before_its_first_turn() {
        let hold = Arc::new(Hold::new(true).unwrap());
        let (turns, stop) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
        let thread = looping(&hold, &turns, &stop);
        assert!(hold.hold(Duration::from_secs(5)));
        assert_eq!(turns.load(Ordering::Acquire), 0);
        stop.store(true, Ordering::Release);
        hold.hold(Duration::from_secs(5));
        thread.join().unwrap();
    }

    #[test]
    fn a_thread_that_has_returned_is_held_at_once() {
        let hold = Hold::new(false).unwrap();
        drop(Leaving(&hold));
        assert!(hold.hold(Duration::ZERO));
    }

    #[test]
    fn a_thread_that_never_parks_is_not_held() {
        let hold = Hold::new(false).unwrap();
        assert!(!hold.hold(Duration::from_millis(20)));
    }
}
