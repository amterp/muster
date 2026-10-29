//! Each topic's part of a snapshot.
//!
//! A topic's content is two things: an arm here that fills its part of the snapshot and joins
//! the topic while holding the lock its events are published under, and a call to
//! [`super::Api::publish`] wherever what it describes changes, under that same lock. `hub.rs`
//! says why the two have to share a lock.

use muster_api as api;

use super::hub::Joining;
use crate::session::Shared;

pub(super) fn join(
    topic: api::Topic,
    shared: &Shared,
    joining: &Joining,
    snapshot: &mut api::Snapshot,
) {
    match topic {
        // This daemon, which does not change while it runs, so no lock is needed.
        api::Topic::Machines => {
            snapshot.machines.push(shared.api.machine());
            shared.api.join(joining, topic);
        }
        // Under the session's lock, which a pane's events are published under.
        api::Topic::Panes => {
            let session = shared.lock();
            snapshot.panes = session.api_panes();
            shared.api.join(joining, topic);
        }
        api::Topic::Messages | api::Topic::Attention | api::Topic::Unspecified => {
            shared.api.join(joining, topic);
        }
    }
}
