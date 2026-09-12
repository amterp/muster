//! What a caller watching agent states is sent, as the window hears it.
//!
//! The shell is told every change to a pane's agent as it happens, which is how the sidebar
//! repaints. A caller outside the process had nothing like it: it polled `ReadWindow` and diffed
//! the answers, late by its polling interval and blind to a finish and a new turn between two
//! polls (kan a_2M9T8O6dL). This hands such a caller the changes the shell is handed.
//!
//! Registered before anything is read. A watch registers, then takes its first picture of the
//! panes, so every change lands in that picture or in the channel after it and none falls between
//! the two. A change can land in both; [`Watch`] drops what it has already sent.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use muster_core::diagnostics::poison;
use muster_core::{AgentState, PaneKey};

use crate::convert;
use crate::proto::{self, Response, response};
use crate::session::{self, PaneAgent};

/// Something a watch may have to say.
#[derive(Debug, Clone)]
pub(crate) enum Seen {
    /// What a pane's agent is doing, as the window paints it.
    State(PaneAgent),
    /// A pane is gone.
    Closed(PaneKey),
}

#[derive(Debug)]
struct Watcher {
    id: u64,
    changes: Sender<Seen>,
}

/// Every watch that is open, and how to reach it.
static WATCHERS: Mutex<Vec<Watcher>> = Mutex::new(Vec::new());

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// What the lock above is called when it is reported recovered.
const WHAT: &str = "watchers";

/// Hands a change to every open watch.
///
/// A watch whose caller has gone is dropped here if it has not already dropped itself. Sending on
/// an unbounded channel never blocks, so a slow caller costs its own watch memory rather than
/// holding up the window that is publishing.
pub(crate) fn publish(seen: &Seen) {
    poison::lock(&WATCHERS, WHAT).retain(|watcher| watcher.changes.send(seen.clone()).is_ok());
}

/// Ends every watch, for a session being reset.
///
/// Each one hears its channel close and hangs up on its caller, which is what a caller of a
/// window that quit hears too.
pub(crate) fn forget_everyone() {
    poison::lock(&WATCHERS, WHAT).clear();
}

/// How many watches are open, for a test proving a caller that hangs up is let go.
pub(crate) fn count() -> usize {
    poison::lock(&WATCHERS, WHAT).len()
}

/// What a watch has to hand its caller next.
#[derive(Debug)]
pub(crate) enum Next {
    /// Send this, and keep watching.
    Answer(Response),
    /// Send this, and hang up: it is what the watch was waiting for, or why it never will be.
    Last(Response),
    /// Nothing happened for a while. Worth checking whether the caller is still there.
    Quiet,
    /// The window is going away, or this watch already sent its last answer.
    Over,
}

/// One caller's watch.
#[derive(Debug)]
pub(crate) struct Watch {
    id: u64,
    changes: Receiver<Seen>,
    /// The panes named, or `None` for every pane including ones that appear later.
    panes: Option<BTreeSet<PaneKey>>,
    until: Vec<AgentState>,
    /// What was last sent about each pane, so a change heard twice is sent once.
    sent: BTreeMap<PaneKey, (AgentState, i64)>,
    ready: VecDeque<Next>,
    ended: bool,
}

/// Starts a watch on `panes`, or on every pane, that ends when one gets to `until`.
///
/// The names are already resolved: a pane nobody holds is refused before this is called.
pub(crate) fn start(panes: Option<BTreeSet<PaneKey>>, until: Vec<AgentState>) -> Watch {
    let (sender, changes) = mpsc::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    poison::lock(&WATCHERS, WHAT).push(Watcher { id, changes: sender });

    let mut watch = Watch {
        id,
        changes,
        panes,
        until,
        sent: BTreeMap::new(),
        ready: VecDeque::new(),
        ended: false,
    };
    watch.begin(session::agents());
    watch
}

impl Watch {
    /// The first picture: every watched pane as it stands.
    ///
    /// A watch with nothing to wait for says all of it. A watch waiting for a state says only the
    /// panes already there, and ends at once if there are any - a condition that already holds
    /// is not waited past.
    fn begin(&mut self, now: Vec<PaneAgent>) {
        let watched: Vec<PaneAgent> =
            now.into_iter().filter(|agent| self.watches(&agent.pane)).collect();
        for agent in &watched {
            self.sent.insert(agent.pane.clone(), (agent.state, agent.since_ms));
        }

        if self.until.is_empty() {
            for agent in &watched {
                self.ready.push_back(Next::Answer(state_answer(agent)));
            }
        } else {
            let arrived: Vec<&PaneAgent> =
                watched.iter().filter(|agent| self.arrived(agent.state)).collect();
            if !arrived.is_empty() {
                for agent in arrived {
                    self.ready.push_back(Next::Answer(state_answer(agent)));
                }
                self.ready.push_back(Next::Last(Response::ok()));
                return;
            }
        }

        // A named pane that closed between being resolved and this picture being taken.
        let missing: Vec<PaneKey> = self
            .panes
            .iter()
            .flatten()
            .filter(|pane| !self.sent.contains_key(*pane))
            .cloned()
            .collect();
        for pane in missing {
            if let Some(next) = self.closed(&pane) {
                self.ready.push_back(next);
            }
        }
    }

    /// The next thing to send, waiting up to `quiet` for one.
    pub(crate) fn next(&mut self, quiet: Duration) -> Next {
        if let Some(next) = self.ready.pop_front() {
            return self.handed(next);
        }
        if self.ended {
            return Next::Over;
        }
        let heard = match self.changes.recv_timeout(quiet) {
            Ok(Seen::State(agent)) => self.state(&agent),
            Ok(Seen::Closed(pane)) => self.closed(&pane),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Next::Over,
        };
        // A change that is not this watch's is as good a moment as a silence to check on the
        // caller.
        heard.map_or(Next::Quiet, |next| self.handed(next))
    }

    /// Notes a last answer on its way out, so nothing is sent after it.
    fn handed(&mut self, next: Next) -> Next {
        if matches!(next, Next::Last(_)) {
            self.ended = true;
            self.ready.clear();
        }
        next
    }

    fn state(&mut self, agent: &PaneAgent) -> Option<Next> {
        if !self.watches(&agent.pane) {
            return None;
        }
        let now = (agent.state, agent.since_ms);
        if self.sent.insert(agent.pane.clone(), now) == Some(now) {
            return None;
        }
        if self.until.is_empty() {
            return Some(Next::Answer(state_answer(agent)));
        }
        if !self.arrived(agent.state) {
            return None;
        }
        self.ready.push_back(Next::Last(Response::ok()));
        Some(Next::Answer(state_answer(agent)))
    }

    fn closed(&mut self, pane: &PaneKey) -> Option<Next> {
        let named = self.panes.as_ref().is_some_and(|panes| panes.contains(pane));
        let was_sent = self.sent.remove(pane).is_some();
        if self.until.is_empty() {
            return (named || was_sent).then(|| {
                Next::Answer(Response {
                    payload: Some(response::Payload::PaneClosed(proto::PaneClosed {
                        daemon_id: pane.daemon.to_string(),
                        pane_id: pane.pane.to_string(),
                    })),
                })
            });
        }
        // Waiting on every pane, one closing is one fewer that could get there, and not a reason
        // to stop. Waiting on this one, it never will.
        named.then(|| {
            Next::Last(Response::failure(format!(
                "pane {} closed before it was {}, so there is nothing left to wait for. Whatever \
                 was running in it ended, or somebody closed it; `muster window` lists what the \
                 window still holds.",
                pane.pane,
                spelled(&self.until)
            )))
        })
    }

    fn watches(&self, pane: &PaneKey) -> bool {
        self.panes.as_ref().is_none_or(|panes| panes.contains(pane))
    }

    fn arrived(&self, state: AgentState) -> bool {
        self.until.iter().any(|wanted| state.counts_as(*wanted))
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let id = self.id;
        poison::lock(&WATCHERS, WHAT).retain(|watcher| watcher.id != id);
    }
}

fn state_answer(agent: &PaneAgent) -> Response {
    Response { payload: Some(response::Payload::PaneState(convert::pane_state(agent))) }
}

/// The states a watch is waiting for, as a sentence would list them: `idle or blocked`.
fn spelled(until: &[AgentState]) -> String {
    until.iter().map(|state| state.as_str()).collect::<Vec<_>>().join(" or ")
}
