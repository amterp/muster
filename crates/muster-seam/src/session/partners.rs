use std::collections::BTreeMap;
use std::sync::{Condvar, LazyLock, Mutex, Once};

use muster_core::attention::GroupKey;
use muster_core::composition::PaneKey;
use muster_core::diagnostics::{log, poison};
use muster_core::fields;
use muster_daemon_proto::{msg_answer, msg_request};

use super::{SESSION, announce_groups, answer_as_human, conversation};

#[derive(Debug, Clone)]
pub(super) struct Partner {
    name: String,
    pub(super) pane: Option<PaneKey>,
}

static WAKE: LazyLock<(Mutex<u64>, Condvar)> = LazyLock::new(|| (Mutex::new(0), Condvar::new()));
static START: Once = Once::new();

pub(super) fn dirty() {
    START.call_once(|| {
        std::thread::Builder::new()
            .name("message-partners".into())
            .spawn(run)
            .expect("start message partner worker");
    });
    let mut generation = poison::lock(&WAKE.0, "message-partners");
    *generation = generation.wrapping_add(1);
    WAKE.1.notify_one();
}

fn run() {
    let mut done = 0;
    loop {
        let mut generation = poison::lock(&WAKE.0, "message-partners");
        while *generation == done {
            generation = WAKE.1.wait(generation).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let reading = *generation;
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
        let generation = poison::lock(&WAKE.0, "message-partners");
        if *generation != reading {
            continue;
        }
        {
            let mut session = poison::lock(&SESSION, "session");
            session.partners.retain(|group, _| groups.contains(group));
            for (group, members) in answers {
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
    }
}
