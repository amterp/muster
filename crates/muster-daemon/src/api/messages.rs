//! What the API's subscribers are told of the messages, worked out under the messages lock after
//! every change to them by comparing the service with what was told last, and the pages of a
//! group's log that `msg.history` answers with.

use std::collections::{BTreeMap, BTreeSet};

use muster_api as api;
use muster_msg::{Entry, GroupSummary, Messaging, Policy, Store, What};
use prost::Message;

use super::hub::{Api, Feed};

/// How many of a group's waiting messages an `Unread` lists. A backlog past it is still counted,
/// and keeps every frame bounded however long the human stays away.
const WAITING_LISTED: usize = 500;

/// How many encoded bytes of entries a page of history holds, past its first: a quarter of a
/// frame, so a page of 1 MiB bodies is a handful of them and never near the limit.
const PAGE_BYTES: usize = 4 << 20;

/// What the subscribers were last told.
#[derive(Debug)]
pub(crate) struct Published {
    /// This daemon, as the home of the groups kept here.
    here: api::Home,
    groups: BTreeMap<String, Kept>,
}

#[derive(Debug)]
struct Kept {
    group: api::Group,
    /// While the human is in the group.
    unread: Option<Told>,
}

#[derive(Debug)]
struct Told {
    /// What the unread state was worked out from: the same again cannot have changed it.
    from: (u64, Option<u64>, Policy),
    unread: api::Unread,
}

impl Published {
    pub(crate) fn new(api: &Api) -> Published {
        let machine = api.machine();
        let home = api::Home { daemon: api.source().daemon, machine: machine.name };
        Published { here: home, groups: BTreeMap::new() }
    }

    /// Publishes whatever changed in `service` since the last call: for each group, its entries
    /// since, then a change to it, then the human's unread state, which changes with both.
    pub(crate) fn publish<S: Store>(&mut self, service: &Messaging<S>, api: &Api) {
        let summaries = service.groups();
        let held: BTreeSet<&str> = summaries.iter().map(|summary| summary.name.as_str()).collect();
        let gone: Vec<String> =
            self.groups.keys().filter(|name| !held.contains(name.as_str())).cloned().collect();
        for name in gone {
            if let Some(kept) = self.groups.remove(&name) {
                deleted(api, kept);
            }
        }
        for summary in summaries {
            let place = service.human_place(&summary.name);
            let group = self.group_of(&summary, place);
            let kept = match self.groups.remove(&summary.name) {
                Some(kept) if replaced(&kept.group, &group) => {
                    deleted(api, kept);
                    made(api, service, group)
                }
                Some(mut kept) => {
                    if group.latest > kept.group.latest {
                        logged(api, service, &summary.name, kept.group.latest);
                    }
                    if !same_but_where_read(&kept.group, &group) {
                        let changed = group.clone();
                        api.publish(api::Topic::Messages, None, || {
                            api::event::Event::GroupChanged(api::GroupChanged {
                                group: Some(changed),
                            })
                        });
                    }
                    kept.group = group;
                    kept
                }
                None => made(api, service, group),
            };
            let unread = unread(api, service, &summary, place, kept.unread);
            self.groups.insert(summary.name, Kept { unread, ..kept });
        }
    }

    /// Every group, as last published.
    pub(crate) fn groups(&self) -> Vec<api::Group> {
        self.groups.values().map(|kept| kept.group.clone()).collect()
    }

    /// Each group with messages waiting for the human, as last published.
    pub(crate) fn waiting(&self) -> Vec<api::Unread> {
        let told = self.groups.values().filter_map(|kept| kept.unread.as_ref());
        told.filter(|told| told.unread.count > 0).map(|told| told.unread.clone()).collect()
    }

    /// The page of a group's log `asked` for, or why there is none.
    pub(crate) fn history<S: Store>(
        &self,
        service: &Messaging<S>,
        asked: &api::History,
    ) -> Result<api::Page, String> {
        let group = asked.group.clone();
        let (Some(entries), Some(kept)) = (service.entries(&group), self.groups.get(&group)) else {
            if service.deleted(&group) {
                return Ok(api::Page { group, deleted: true, ..api::Page::default() });
            }
            return Err(format!(
                "this daemon holds no group {group}; the messages topic's snapshot names every \
                 group it holds"
            ));
        };
        let through = if asked.through == 0 { u64::MAX } else { asked.through };
        let range: Vec<&Entry> = entries
            .iter()
            .filter(|entry| entry.seq > asked.after && entry.seq <= through)
            .collect();
        let answers = answers(entries);
        let mut page = Vec::new();
        let mut bytes = 0;
        for entry in range.iter().rev() {
            let entry = entry_of(entry, answers.get(&entry.seq).copied());
            // A field's tag and length come to a few bytes past the entry's own.
            bytes += entry.encoded_len() + 8;
            if !page.is_empty() && bytes > PAGE_BYTES {
                break;
            }
            page.push(entry);
        }
        let more = page.len() < range.len();
        page.reverse();
        Ok(api::Page {
            group,
            entries: page,
            more,
            held_after: kept.group.held_after,
            deleted: false,
        })
    }

    fn group_of(&self, summary: &GroupSummary, place: Option<u64>) -> api::Group {
        let home = match &summary.home {
            Some(machine) => api::Home { daemon: String::new(), machine: machine.clone() },
            None => self.here.clone(),
        };
        api::Group {
            name: summary.name.clone(),
            home: Some(home),
            members: summary.members.clone(),
            policy: Some(policy_of(&summary.policy)),
            paused: summary.policy.paused,
            latest: summary.latest,
            human_place: place,
            held_after: summary.held_after,
        }
    }
}

/// Whether `now` is another group under the name `before` had: one deleted and made again, or a
/// replica its home started over.
fn replaced(before: &api::Group, now: &api::Group) -> bool {
    now.held_after != before.held_after || now.latest < before.latest
}

/// Whether two states of a group differ only in what Logged and Unread events move.
fn same_but_where_read(before: &api::Group, now: &api::Group) -> bool {
    before.name == now.name
        && before.home == now.home
        && before.members == now.members
        && before.policy == now.policy
        && before.paused == now.paused
        && before.held_after == now.held_after
}

/// Tells the subscribers of a group that appeared, then of each of its entries, so that a post
/// that made its group arrives with its body like any other.
fn made<S: Store>(api: &Api, service: &Messaging<S>, group: api::Group) -> Kept {
    let before_its_entries = api::Group { latest: group.held_after, ..group.clone() };
    api.publish(api::Topic::Messages, None, || {
        api::event::Event::GroupMade(api::GroupMade { group: Some(before_its_entries) })
    });
    logged(api, service, &group.name, group.held_after);
    Kept { group, unread: None }
}

/// Tells the subscribers `kept` is gone, and that nothing waits for the human there.
fn deleted(api: &Api, kept: Kept) {
    let name = kept.group.name;
    let group = name.clone();
    api.publish(api::Topic::Messages, None, || {
        api::event::Event::GroupDeleted(api::GroupDeleted { group })
    });
    if kept.unread.is_some() {
        let unread = api::Unread { group: name, ..api::Unread::default() };
        api.publish(Feed::Unread, None, || api::event::Event::Unread(unread));
    }
}

/// Publishes each entry of `group` after `after`.
fn logged<S: Store>(api: &Api, service: &Messaging<S>, group: &str, after: u64) {
    let entries = service.entries(group).unwrap_or_default();
    let new = entries.partition_point(|entry| entry.seq <= after);
    for entry in &entries[new..] {
        api.publish(api::Topic::Messages, None, || {
            api::event::Event::Logged(api::Logged {
                group: group.to_string(),
                entry: Some(entry_of(entry, None)),
            })
        });
    }
}

/// The human's unread state in a group, published when it changed since `told`.
fn unread<S: Store>(
    api: &Api,
    service: &Messaging<S>,
    summary: &GroupSummary,
    place: Option<u64>,
    told: Option<Told>,
) -> Option<Told> {
    if place.is_none() && told.is_none() {
        return None;
    }
    let from = (summary.latest, place, summary.policy.clone());
    if let Some(told) = told.as_ref().filter(|told| told.from == from) {
        return Some(Told { from, unread: told.unread.clone() });
    }
    let unread = unread_of(service, &summary.name, place);
    if told.is_none_or(|told| told.unread != unread) {
        let event = unread.clone();
        api.publish(Feed::Unread, None, || api::event::Event::Unread(event));
    }
    place.map(|_| Told { from, unread })
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

/// The post answering each question in `entries`, by the question's number. A question takes
/// one answer, so the first is the one.
fn answers(entries: &[Entry]) -> BTreeMap<u64, u64> {
    let mut answers = BTreeMap::new();
    for entry in entries {
        if let What::Message { answering: Some(answering), .. } = &entry.what {
            answers.entry(answering.question).or_insert(entry.seq);
        }
    }
    answers
}

fn entry_of(entry: &Entry, answered_by: Option<u64>) -> api::Entry {
    use api::entry::What as Is;
    let what = match &entry.what {
        What::Message { author, to, body, urgent, answers, answering } => Is::Post(api::Post {
            author: author.clone(),
            to: to.clone(),
            body: body.clone(),
            urgent: *urgent,
            answers: answers.clone(),
            answering: answering.as_ref().map(|answering| api::Answering {
                question: answering.question,
                answer: answering.answer.clone(),
            }),
            answered_by,
        }),
        What::Created { by } => Is::Made(api::Made { by: by.clone() }),
        What::Joined { who } => Is::Joined(api::Joined { who: who.clone() }),
        What::Left { who } => Is::Left(api::Left { who: who.clone() }),
        What::Changed { by, change, policy } => Is::Changed(api::Changed {
            by: by.clone(),
            change: match change {
                muster_msg::Change::SetPolicy => api::Change::PolicySet,
                muster_msg::Change::Paused => api::Change::Paused,
                muster_msg::Change::Resumed => api::Change::Resumed,
            }
            .into(),
            policy: policy.as_deref().map(policy_of),
        }),
    };
    api::Entry { number: entry.seq, at_ms: entry.at_ms, what: Some(what) }
}

fn policy_of(policy: &Policy) -> api::Policy {
    let names = |sets: &BTreeMap<String, Vec<String>>| {
        sets.iter()
            .map(|(author, names)| (author.clone(), api::Names { names: names.clone() }))
            .collect()
    };
    api::Policy {
        ring: names(&policy.ring),
        allow: names(&policy.allow),
        membership: policy.membership.clone(),
        urgent: policy.urgent.clone(),
    }
}
