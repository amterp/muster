//! Whether this bridge holds its pane's herdr client, and the handover between the threads that
//! decide it.
//!
//! A pane nobody can see costs its daemon a render per pass for as long as a client is attached
//! (herdr renders every client on the thread that also answers requests), so a bridge told its
//! pane is hidden lets its client go - parks - and keeps everything else: the surface keeps its
//! last picture, the app keeps its socket, and herdr keeps the pane at the size this bridge gave
//! it (`observations/herdr-0.8.0.md` section 4). Shown again, it starts a client, whose first
//! frame is a full repaint.
//!
//! Input still reaches a parked pane: `muster pane send` to a pane behind another tab is
//! ordinary. It brings a client back for as long as input keeps arriving, and the pane parks
//! again after [`LINGER`] without any.
//!
//! Three threads meet here. The relay writes what the app sends and passes on whether the pane
//! is shown; the pump reads frames and is the one that sees a client end and starts the next;
//! the resize watcher writes geometry, which a parked pane does not need - the next client is
//! started at the size the surface is then.

use std::io::Write;
use std::process::ChildStdin;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use muster_core::diagnostics::{log, poison};
use muster_core::fields;

/// How long a hidden pane that was typed into keeps its client after the last input.
///
/// Long enough that a burst of `muster pane send` calls, each waiting on the pane's answer, does
/// not pay a reattach apiece; short beside how long a pane stays hidden.
pub(crate) const LINGER: Duration = Duration::from_secs(10);

pub(crate) struct Attachment {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    /// The running client's stdin, and `None` while there is no client to write to.
    input: Option<ChildStdin>,
    phase: Phase,
    /// Whether the app last said this pane is on screen. True until it says otherwise, because
    /// a bridge is started for a pane a region is showing.
    shown: bool,
    /// When input for this pane last arrived, which keeps a hidden pane streaming a while.
    typed_at: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// A client is running and writable.
    Streaming,
    /// Its stdin has been closed, which is how a herdr client is told to detach, and the pump
    /// has yet to see it go.
    Stopping,
    /// No client, until one is wanted.
    Parked,
}

/// What the pump should do about a client whose stream just ended.
pub(crate) enum Ended {
    /// It was let go on purpose, and a client is wanted again: start one.
    Restart { because: &'static str },
    /// It went on its own. The bridge's life ends with it, as it always has.
    Finished,
}

impl Attachment {
    pub(crate) fn new(input: ChildStdin) -> Attachment {
        Attachment {
            state: Mutex::new(State {
                input: Some(input),
                phase: Phase::Streaming,
                shown: true,
                typed_at: None,
            }),
            changed: Condvar::new(),
        }
    }

    /// Writes input for the pane, starting a client first if it is parked.
    ///
    /// Blocks until there is a client to take it, which is the relay's own thread: nothing it
    /// could do meanwhile would reach the pane any sooner.
    pub(crate) fn send_input(&self, line: &[u8]) -> bool {
        let mut state = self.lock();
        state.typed_at = Some(Instant::now());
        self.changed.notify_all();
        while state.phase != Phase::Streaming {
            state = poison::recover(self.changed.wait(state), "bridge.attachment");
        }
        write(&mut state, line)
    }

    /// Writes to the client if there is one, and drops the message if not.
    ///
    /// For geometry: a parked pane needs no resize, because the next client is started at the
    /// size the surface is then.
    pub(crate) fn send_if_streaming(&self, line: &[u8]) {
        let mut state = self.lock();
        if state.phase == Phase::Streaming {
            write(&mut state, line);
        }
    }

    /// Takes the app's word on whether this pane is on screen.
    pub(crate) fn show(&self, shown: bool) {
        let mut state = self.lock();
        state.shown = shown;
        if !shown {
            park_if_unwanted(&mut state);
        }
        self.changed.notify_all();
    }

    /// Parks a hidden pane once its linger has run out, until the bridge ends.
    ///
    /// Its own thread, because the moment to park is a moment nothing else is doing anything.
    pub(crate) fn park_when_idle(&self) -> ! {
        let mut state = self.lock();
        loop {
            park_if_unwanted(&mut state);
            state =
                poison::recover(self.changed.wait_timeout(state, LINGER), "bridge.attachment").0;
        }
    }

    /// Called by the pump when its client's stream ends, and blocks until a client is wanted if
    /// the stream ended because this let it go.
    pub(crate) fn ended(&self) -> Ended {
        let mut state = self.lock();
        if state.phase == Phase::Streaming {
            return Ended::Finished;
        }
        state.phase = Phase::Parked;
        log::info("bridge.parked", fields! {});
        self.changed.notify_all();
        loop {
            if state.shown {
                return Ended::Restart { because: "shown" };
            }
            if typed_recently(&state) {
                return Ended::Restart { because: "input" };
            }
            state = poison::recover(self.changed.wait(state), "bridge.attachment");
        }
    }

    /// A new client is running: writes may go to it.
    pub(crate) fn attached(&self, input: ChildStdin) {
        let mut state = self.lock();
        state.input = Some(input);
        state.phase = Phase::Streaming;
        park_if_unwanted(&mut state);
        self.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        poison::lock(&self.state, "bridge.attachment")
    }
}

fn park_if_unwanted(state: &mut State) {
    if state.phase != Phase::Streaming || state.shown || typed_recently(state) {
        return;
    }
    // Closing a client's stdin is how it is told to detach: it sends herdr a release and
    // exits, and the pump sees its stream end.
    state.input = None;
    state.phase = Phase::Stopping;
}

fn typed_recently(state: &State) -> bool {
    state.typed_at.is_some_and(|at| at.elapsed() < LINGER)
}

fn write(state: &mut State, line: &[u8]) -> bool {
    let Some(input) = state.input.as_mut() else { return false };
    // Nowhere useful to report a failed write: herdr has gone, and the pump is about to notice
    // and say so with the reason it was given.
    input.write_all(line).and_then(|()| input.flush()).is_ok()
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;

    use super::*;

    fn cat() -> Child {
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cat runs everywhere this does")
    }

    #[test]
    fn input_for_a_parked_pane_waits_for_a_client_and_then_reaches_it() {
        let mut first = cat();
        let attachment = Arc::new(Attachment::new(first.stdin.take().expect("piped")));
        attachment.show(false);

        // The pump's side: the client it was reading from has gone, and it waits for a reason
        // to start another.
        let pump = std::thread::spawn({
            let attachment = Arc::clone(&attachment);
            move || matches!(attachment.ended(), Ended::Restart { because: "input" })
        });
        let typist = std::thread::spawn({
            let attachment = Arc::clone(&attachment);
            move || attachment.send_input(b"hello\n")
        });

        assert!(pump.join().expect("the pump thread"), "input should be why a client restarted");
        let mut second = cat();
        attachment.attached(second.stdin.take().expect("piped"));
        assert!(typist.join().expect("the typist thread"), "the input should have been written");

        let mut echoed = [0u8; 6];
        second.stdout.take().expect("piped").read_exact(&mut echoed).expect("cat echoes");
        assert_eq!(&echoed, b"hello\n", "the input went to the new client, not the old one");

        // Hidden and typed into, it keeps its client for a while rather than parking at once.
        attachment.show(false);
        assert_eq!(poison::lock(&attachment.state, "test").phase, Phase::Streaming);

        for mut child in [first, second] {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
