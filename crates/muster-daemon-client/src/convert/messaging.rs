//! The daemon's messaging answers in the core's words: a group's log, its members' liveness, its
//! policy, and whom a post reached. Every view of a group - the banner and the conversation alike
//! - reads them through here, so a field the daemon adds reaches all of them or none.

use muster_core::conversation::{Answering, Change, Entry, Liveness, Message, Policy, What};
use muster_core::posting::{Reach, Reached};
use muster_daemon_proto::{MsgAnswer, msg_answer, msg_request};

/// What a log answered for one group.
#[derive(Debug, Default)]
pub struct LogPage {
    pub entries: Vec<Entry>,
    /// The number of the last entry sent, including any of a kind this build leaves out.
    pub last: u64,
    /// The daemon left out entries before these.
    pub earlier: bool,
    /// The machine the group is kept on, while it cannot be reached.
    pub behind: Option<String>,
}

/// The entries `answer` holds for `group`. An answer of any other kind holds none.
pub fn log_page(answer: &MsgAnswer, group: &str) -> LogPage {
    let mut page = LogPage::default();
    let Some(msg_answer::Answer::Entries(answered)) = &answer.answer else { return page };
    for logged in answered.groups.iter().filter(|logged| logged.group == group) {
        page.earlier |= logged.earlier;
        page.behind.clone_from(&logged.behind);
        page.last = logged.entries.iter().map(|logged| logged.seq).fold(page.last, u64::max);
        page.entries.extend(logged.entries.iter().filter_map(entry));
    }
    page
}

/// One entry of a group's log, or nothing for a kind of entry this build does not know.
pub fn entry(entry: &msg_answer::Entry) -> Option<Entry> {
    use msg_answer::entry::{Change as Wire, What as Logged};
    let what = match entry.what.as_ref()? {
        Logged::Message(message) => What::Message(Message {
            answers: message.answers.clone(),
            answering: message.answering.as_ref().map(|answering| Answering {
                question: answering.question,
                answer: answering.answer.clone(),
            }),
            ..Message::new(
                message.author.clone(),
                message.to.clone(),
                message.body.clone(),
                message.urgent,
            )
        }),
        Logged::Created(name) => What::Created(name.clone()),
        Logged::Joined(name) => What::Joined(name.clone()),
        Logged::Left(name) => What::Left(name.clone()),
        Logged::Changed(changed) => What::Changed {
            by: changed.by.clone(),
            change: match changed.change() {
                Wire::Paused => Change::Paused,
                Wire::Resumed => Change::Resumed,
                Wire::SetPolicy | Wire::Unspecified => Change::Policy,
            },
        },
    };
    Some(Entry { seq: entry.seq, at_ms: entry.at_ms, what })
}

pub fn liveness(liveness: msg_answer::Liveness) -> Liveness {
    match liveness {
        msg_answer::Liveness::Alive | msg_answer::Liveness::Unspecified => Liveness::Alive,
        msg_answer::Liveness::Gone => Liveness::Gone,
        msg_answer::Liveness::Human => Liveness::Human,
        msg_answer::Liveness::Unreachable => Liveness::Unreachable,
    }
}

/// The parts of a group's policy a conversation reads.
pub fn policy(policy: msg_request::Policy) -> Policy {
    Policy {
        ring: policy.ring.into_iter().map(|(author, names)| (author, names.names)).collect(),
        paused: policy.paused,
    }
}

pub fn reached(reached: &msg_answer::Reached) -> Reached {
    use msg_answer::Reach as Wire;
    let reach = match reached.reach() {
        Wire::Woken => Reach::Woken,
        Wire::AlreadyWoken => Reach::AlreadyWoken,
        // A reach this build is too old to know is one it cannot claim anybody heard.
        Wire::Waiting | Wire::Unspecified => Reach::Waiting,
        Wire::Gone => Reach::Gone,
        Wire::Deferred => Reach::Deferred,
        Wire::NoAgent => Reach::NoAgent,
        Wire::NoDoorbell => Reach::NoDoorbell,
        Wire::Paused => Reach::Paused,
        Wire::Unreachable => Reach::Unreachable,
    };
    Reached { name: reached.name.clone(), reach }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_of_a_kind_this_build_does_not_know_still_counts_as_sent() {
        let answer = MsgAnswer {
            answer: Some(msg_answer::Answer::Entries(msg_answer::Entries {
                groups: vec![msg_answer::GroupEntries {
                    group: "g".to_string(),
                    entries: vec![msg_answer::Entry { seq: 7, what: None, ..Default::default() }],
                    ..Default::default()
                }],
            })),
            ..Default::default()
        };
        let page = log_page(&answer, "g");
        assert!(page.entries.is_empty(), "nothing to draw");
        assert_eq!(page.last, 7, "but the follow goes on from after it");
    }

    #[test]
    fn a_question_and_its_answer_keep_what_they_offer_and_answer() {
        let message = |answers: Vec<String>, answering| msg_answer::Entry {
            seq: 4,
            what: Some(msg_answer::entry::What::Message(msg_answer::Message {
                author: "otter".to_string(),
                body: "Run it?".to_string(),
                answers,
                answering,
                ..Default::default()
            })),
            ..Default::default()
        };
        let question = entry(&message(vec!["Yes".into(), "No".into()], None)).unwrap();
        let What::Message(question) = question.what else { panic!("not a message") };
        assert_eq!(question.answers, ["Yes", "No"]);
        let answering = msg_request::Answering { question: 3, answer: "Yes".to_string() };
        let reply = entry(&message(Vec::new(), Some(answering))).unwrap();
        let What::Message(reply) = reply.what else { panic!("not a message") };
        assert_eq!(reply.answering, Some(Answering { question: 3, answer: "Yes".to_string() }));
    }
}
