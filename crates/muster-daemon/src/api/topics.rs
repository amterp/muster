//! Each topic's part of a snapshot.
//!
//! A topic's content is two things: filling its part of the snapshot and joining its feed while
//! holding the lock its events are published under, and a call to [`super::Api::publish`]
//! wherever what it describes changes, under that same lock. `hub.rs` says why the two have to
//! share a lock. Every part published under one lock is filled in one acquisition of it, so a
//! snapshot never holds two of them from different moments: the human's place in a group and
//! what waits for them there are both under the messages lock, and agree.

use std::collections::BTreeSet;

use muster_api as api;
use muster_daemon_proto as proto;
use proto::event::Event as Payload;

use super::hub::{Feed, Joining};
use crate::session::{Session, Shared};

/// The parts published under the session's lock, which `session` holds.
pub(super) fn join_under_session(
    session: &Session,
    shared: &Shared,
    joining: &Joining,
    topics: &BTreeSet<api::Topic>,
    snapshot: &mut api::Snapshot,
) {
    if topics.contains(&api::Topic::Attention) {
        let (asking, presence) = session.attention_now();
        let attention = snapshot.attention.get_or_insert_default();
        attention.asking = asking.iter().map(asking_of).collect();
        attention.presence = Some(presence_of(&presence));
        shared.api.join(joining, api::Topic::Attention);
    }
    if topics.contains(&api::Topic::Panes) {
        snapshot.panes = session.api_panes();
        shared.api.join(joining, api::Topic::Panes);
    }
}

/// The parts published under the messages lock, in one acquisition of it.
pub(super) fn join_under_messages(
    shared: &Shared,
    joining: &Joining,
    topics: &BTreeSet<api::Topic>,
    snapshot: &mut api::Snapshot,
) {
    let hears = |topic| topics.contains(&topic);
    if !hears(api::Topic::Messages) && !hears(api::Topic::Attention) {
        return;
    }
    let mut messages = shared.messages();
    let published = messages.published();
    if hears(api::Topic::Attention) {
        snapshot.attention.get_or_insert_default().unread = published.waiting();
    }
    if hears(api::Topic::Messages) {
        shared.api.join(joining, api::Topic::Messages);
    }
    shared.api.join(joining, Feed::Unread);
}

/// The parts that change under no lock.
pub(super) fn join_unlocked(
    shared: &Shared,
    joining: &Joining,
    topics: &BTreeSet<api::Topic>,
    snapshot: &mut api::Snapshot,
) {
    // This daemon, which does not change while it runs.
    if topics.contains(&api::Topic::Machines) {
        snapshot.machines.push(shared.api.machine());
        shared.api.join(joining, api::Topic::Machines);
    }
}

/// The attention topic's event for what the session just emitted, when it is one.
pub(crate) fn attention_event(payload: &Payload) -> Option<api::event::Event> {
    match payload {
        Payload::Asking(asking) => Some(api::event::Event::Asking(asking_of(asking))),
        Payload::Presence(presence) => Some(api::event::Event::Presence(presence_of(presence))),
        Payload::Shown(shown) => Some(api::event::Event::Shown(shown_of(shown))),
        _ => None,
    }
}

fn asking_of(asking: &proto::Asking) -> api::Asking {
    let ask = match asking.ask() {
        proto::Ask::Blocked => api::Ask::Blocked,
        proto::Ask::Stuck => api::Ask::Stuck,
        proto::Ask::Unspecified => api::Ask::Unspecified,
    };
    api::Asking {
        pane: asking.pane.clone(),
        ask: ask.into(),
        unheard_helper: asking.unheard_helper,
    }
}

fn presence_of(presence: &proto::Presence) -> api::Presence {
    api::Presence {
        attended: presence.attended,
        known: presence.known,
        in_front: presence.in_front,
        last_input_ms: presence.last_input_ms,
    }
}

fn shown_of(shown: &proto::Shown) -> api::Shown {
    let about = shown.about.as_ref().map(|about| match about {
        proto::shown::About::Pane(pane) => api::shown::About::Pane(pane.clone()),
        proto::shown::About::Group(group) => api::shown::About::Group(group.clone()),
    });
    let shown_as = match shown.shown_as() {
        proto::ShownAs::Banner => api::ShownAs::Banner,
        proto::ShownAs::Notification => api::ShownAs::Notification,
        proto::ShownAs::Unspecified => api::ShownAs::Unspecified,
    };
    api::Shown { about, shown_as: shown_as.into(), why: shown.why.clone() }
}
