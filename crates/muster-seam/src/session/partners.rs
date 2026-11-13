use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Condvar, LazyLock, Mutex, Once};

use muster_core::attention::GroupKey;
use muster_core::composition::{DaemonId, PaneKey};
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto::{msg_answer, msg_request};

use super::{SESSION, announce_groups, answer_as_human, conversation};

#[derive(Debug, Clone)]
pub(super) struct Partner {
    name: String,
    pub(super) pane: Option<PaneKey>,
}

#[derive(Default)]
struct Revisions {
    next: u64,
    observed: BTreeMap<DaemonId, u64>,
    notices: BTreeMap<GroupKey, u64>,
    completed: u64,
}

static WAKE: LazyLock<(Mutex<Revisions>, Condvar)> =
    LazyLock::new(|| (Mutex::new(Revisions::default()), Condvar::new()));
static START: Once = Once::new();

pub(super) fn dirty() {
    wake();
}

pub(super) fn noticed(group: &GroupKey) {
    wake();
    let mut revisions = poison::lock(&WAKE.0, "message-partners");
    let revision = revisions.next;
    revisions.notices.insert(group.clone(), revision);
}

pub(super) fn notice_progress(daemon: &str, group: &str) -> u64 {
    let revisions = poison::lock(&WAKE.0, "message-partners");
    revisions
        .notices
        .get(&GroupKey { daemon: DaemonId::new(daemon), group: group.into() })
        .copied()
        .unwrap_or(0)
}

pub(super) fn changed_on(daemon: &DaemonId) {
    wake();
    let mut revisions = poison::lock(&WAKE.0, "message-partners");
    let revision = revisions.next;
    revisions.observed.insert(daemon.clone(), revision);
}

pub(super) fn progress(daemon: &str) -> (u64, u64) {
    let revisions = poison::lock(&WAKE.0, "message-partners");
    (revisions.observed.get(&DaemonId::new(daemon)).copied().unwrap_or(0), revisions.completed)
}

fn wake() {
    START.call_once(|| {
        if let Err(error) = std::thread::Builder::new().name("message-partners".into()).spawn(run) {
            log::warn("partners.worker_failed", fields! {
                "error" => error.to_string(),
                "impact" => "conversation partner placement cannot refresh until Muster restarts",
                "check" => "whether the machine has run out of threads",
            });
        }
    });
    let mut generation = poison::lock(&WAKE.0, "message-partners");
    generation.next = generation.next.wrapping_add(1);
    WAKE.1.notify_one();
}

fn run() {
    let mut done = 0;
    loop {
        let mut generation = poison::lock(&WAKE.0, "message-partners");
        while generation.next == done {
            generation = WAKE.1.wait(generation).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let reading = generation.next;
        drop(generation);
        let groups: Vec<GroupKey> = {
            let session = poison::lock(&SESSION, "session");
            session
                .backends
                .iter()
                .flat_map(|(daemon, backend)| {
                    let mirror = poison::lock(&backend.mirror, "mirror");
                    mirror
                        .human_notices()
                        .filter(|(_, notice)| notice.listed())
                        .map(|(group, _)| GroupKey { daemon: daemon.clone(), group: group.clone() })
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let mut answers = BTreeMap::new();
        for group in &groups {
            let request =
                msg_request::Request::Who(msg_request::Who { group: Some(group.group.clone()) });
            match answer_as_human(&group.daemon, request) {
                Ok(answer) => {
                    if let Some(msg_answer::Answer::Members(members)) = answer.answer {
                        answers.insert(group.clone(), members.members);
                    }
                }
                Err(refusal) => log::info(
                    "partners.unknown",
                    fields! {
                        "daemon" => group.daemon.to_string(), "group" => &group.group,
                        "why" => refusal.detail(),
                    },
                ),
            }
        }
        let mut generation = poison::lock(&WAKE.0, "message-partners");
        {
            let mut session = poison::lock(&SESSION, "session");
            let current: BTreeSet<GroupKey> = session
                .backends
                .iter()
                .flat_map(|(daemon, backend)| {
                    let mirror = poison::lock(&backend.mirror, "mirror");
                    mirror
                        .human_notices()
                        .filter(|(_, notice)| notice.listed())
                        .map(|(group, _)| GroupKey { daemon: daemon.clone(), group: group.clone() })
                        .collect::<Vec<_>>()
                })
                .collect();
            session.partners.retain(|group, _| current.contains(group));
            generation.notices.retain(|group, _| current.contains(group));
            for (group, members) in answers {
                if !current.contains(&group) {
                    continue;
                }
                let human = |name: &str| name == "@human" || name.starts_with("@human@");
                let partner = if members.len() == 2
                    && members.iter().filter(|member| human(&member.name)).count() == 1
                {
                    members.into_iter().find(|member| !human(&member.name))
                } else {
                    None
                };
                if let Some(member) = partner {
                    let pane = if member.liveness() == msg_answer::Liveness::Unreachable {
                        session
                            .partners
                            .get(&group)
                            .filter(|old| old.name == member.name)
                            .and_then(|old| old.pane.clone())
                    } else {
                        member
                            .pane
                            .as_deref()
                            .and_then(|pane| conversation::pane_of(&session, &group, pane))
                    };
                    session.partners.insert(group, Partner { name: member.name, pane });
                } else {
                    session.partners.remove(&group);
                }
            }
        }
        done = reading;
        drop(generation);
        announce_groups();
        let windows = poison::lock(&SESSION, "session").windows.opened();
        conversation::announce(&windows);
        poison::lock(&WAKE.0, "message-partners").completed = reading;
    }
}
