//! What the API's subscribers are told of the messages, worked out under the messages lock after
//! every change to them by comparing the service with what was told last.

use std::collections::BTreeMap;

use muster_api as api;
use muster_msg::{Messaging, Policy, Store};

use super::hub::{Api, Feed};

/// How many of a group's waiting messages an `Unread` lists. A backlog past it is still counted,
/// and keeps every frame bounded however long the human stays away.
const WAITING_LISTED: usize = 500;

/// What the subscribers were last told.
#[derive(Debug, Default)]
pub(crate) struct Published {
    /// Each group the human is in, by name.
    unread: BTreeMap<String, Told>,
}

#[derive(Debug)]
struct Told {
    /// What the unread state was worked out from: the same again cannot have changed it.
    from: (u64, Option<u64>, Policy),
    unread: api::Unread,
}

impl Published {
    /// Publishes whatever changed in `service` since the last call.
    pub(crate) fn publish<S: Store>(&mut self, service: &Messaging<S>, api: &Api) {
        let mut gone: Vec<String> = self.unread.keys().cloned().collect();
        for group in service.groups() {
            let place = service.human_place(&group.name);
            gone.retain(|name| *name != group.name);
            let told = self.unread.get(&group.name);
            if place.is_none() && told.is_none() {
                continue;
            }
            let from = (group.latest, place, group.policy);
            if told.is_some_and(|told| told.from == from) {
                continue;
            }
            let unread = unread_of(service, &group.name, place);
            if told.is_none_or(|told| told.unread != unread) {
                let event = unread.clone();
                api.publish(Feed::Unread, None, || api::event::Event::Unread(event));
            }
            if place.is_some() {
                self.unread.insert(group.name, Told { from, unread });
            } else {
                self.unread.remove(&group.name);
            }
        }
        for group in gone {
            self.unread.remove(&group);
            let unread = api::Unread { group, ..api::Unread::default() };
            api.publish(Feed::Unread, None, || api::event::Event::Unread(unread));
        }
    }

    /// Each group with messages waiting for the human, as last published.
    pub(crate) fn waiting(&self) -> Vec<api::Unread> {
        self.unread
            .values()
            .filter(|told| told.unread.count > 0)
            .map(|told| told.unread.clone())
            .collect()
    }
}

fn unread_of<S: Store>(service: &Messaging<S>, group: &str, place: Option<u64>) -> api::Unread {
    let waiting = service.human_waiting(group);
    let listed = &waiting[waiting.len().saturating_sub(WAITING_LISTED)..];
    api::Unread {
        group: group.to_string(),
        place,
        count: waiting.len() as u64,
        waiting: listed
            .iter()
            .map(|post| api::WaitingPost {
                number: post.seq,
                at_ms: post.at_ms,
                author: post.author.clone(),
                addressed: post.addressed,
                urgent: post.urgent,
            })
            .collect(),
    }
}
