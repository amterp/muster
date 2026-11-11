//! Whether a participant on another machine would hear a post now, as its machine last said.
//!
//! Muster's notices about a member here wait for whoever they are for to be free: a post to an
//! agent at work is read when its turn ends, and by then what the notice says has often stopped
//! being true. An agent here is judged from its pane. One on another machine is asked about over
//! the link, with `who` in a group the two share, and nothing there wakes this daemon when it
//! comes free, so it is asked again while a notice for it waits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muster_core::diagnostics::poison;
use muster_msg::{Activity, Settled};

use super::presence::Panes;
use super::{now_ms, peer};
use crate::session::Shared;

/// How often a participant elsewhere is asked about while a notice for it waits.
pub(crate) const ASK_BUSY_AGAIN: Duration = Duration::from_secs(5);

/// What a participant's machine last said of it, when it was asked, and whether it is being
/// asked now.
#[derive(Debug, Clone, Copy, Default)]
struct Said {
    busy: bool,
    asked_ms: u64,
    asking: bool,
}

/// What each participant on another machine's machine last said of it, by its name as this
/// machine writes it: written by the threads that ask, read by each look.
#[derive(Debug, Default, Clone)]
pub(crate) struct Busy {
    said: Arc<Mutex<HashMap<String, Said>>>,
}

impl Busy {
    /// Whether `name` would not hear a post now, as far as its machine has said: one never asked
    /// about yet is taken as busy until its machine answers, so nothing is told on a guess.
    pub(crate) fn busy(&self, name: &str) -> bool {
        let said = poison::lock(&self.said, "msg.busy.said");
        said.get(name).is_none_or(|said| said.busy || said.asked_ms == 0)
    }

    /// Asks the machine `name` is on, through `who` in `group`, whether it is at work, at a
    /// dialog or stuck: at most every [`ASK_BUSY_AGAIN`] and one ask per name at a time. The ask
    /// is a call over a link, which can wait seconds, so it runs on a thread of its own and
    /// nudges the doorbell's thread with the answer.
    pub(crate) fn ask(&self, shared: &Arc<Shared>, name: &str, group: Option<String>) {
        let now = now_ms();
        let again_ms = u64::try_from(ASK_BUSY_AGAIN.as_millis()).unwrap_or(u64::MAX);
        {
            let mut said = poison::lock(&self.said, "msg.busy.said");
            let was = said.get(name).copied().unwrap_or_default();
            let fresh = was.asked_ms > 0 && now.saturating_sub(was.asked_ms) < again_ms;
            if fresh || was.asking {
                return;
            }
            said.insert(name.to_string(), Said { asking: true, ..was });
        }
        let (asker, said) = (Arc::clone(shared), Arc::clone(&self.said));
        let started = std::thread::Builder::new().name("msg-busy".to_string()).spawn({
            let name = name.to_string();
            move || {
                let busy = group.is_some_and(|group| busy_there(&asker, &name, &group));
                let answer = Said { busy, asked_ms: now_ms(), asking: false };
                poison::lock(&said, "msg.busy.said").insert(name, answer);
                asker.doorbell.nudge();
            }
        });
        if started.is_err() {
            let answer = Said { busy: false, asked_ms: now, asking: false };
            poison::lock(&self.said, "msg.busy.said").insert(name.to_string(), answer);
        }
    }
}

/// Whether `name`, on another machine, is at work, at a dialog or stuck, as the machines with
/// members in `group` say of it. A group with no such machine says nothing, and it is taken as
/// free.
fn busy_there(shared: &Shared, name: &str, group: &str) -> bool {
    let panes = Panes::of(shared);
    let asked = {
        let messages = shared.messages();
        let service = &messages.service;
        service
            .who(Some(group), &panes)
            .ok()
            .and_then(|members| Some((members, service.route_who(Some(group)).ok()?)))
    };
    let Some((mut members, away)) = asked else { return false };
    for away in away {
        let (settle, holding) = peer::call_away(shared, &away);
        super::ring(shared, holding);
        if let Ok(Settled::Members(heard)) = settle.result {
            muster_msg::heard(&mut members, &away.machine, &heard);
        }
    }
    let activity =
        members.iter().find(|member| member.name == name).and_then(|member| member.activity);
    matches!(activity, Some(Activity::Working | Activity::Blocked | Activity::Stuck))
}
