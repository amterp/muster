//! Posting as the human, from a banner's reply or a conversation's message box: the one request
//! both send, how an unanswered one is settled from the group's log, and how each outcome is
//! answered (mip/0009-group-conversation.md, section 4). What the outcomes mean is the core's
//! (`muster_core::posting`); this asks the daemon.

use muster_core::attention::GroupKey;
use muster_core::conversation::{Answering, Log};
use muster_core::intent::Refusal;
use muster_core::posting::{self, Reached, Sent};
use muster_daemon_client::convert::messaging;
use muster_daemon_proto::msg_answer;
use muster_daemon_proto::msg_request::{self, Request as Asked};

use super::answer_as_human;
use crate::proto::Response;

/// Posts `text` to `to` in `group` as the human, answering a question when `answering` says
/// which, and says what came of it.
///
/// An unanswered post may have been kept, and a post carries no key the daemon could refuse a
/// second copy by, so it is never sent again. The group's log is asked once instead, for the
/// human's entry after `after` with these recipients and these words.
pub(crate) fn post_as_human(
    group: &GroupKey,
    to: &[String],
    text: &str,
    answering: Option<&Answering>,
    urgent: bool,
    after: u64,
) -> Sent {
    let post = msg_request::Post {
        group: Some(group.group.clone()),
        to: to.to_vec(),
        body: text.to_string(),
        urgent,
        answering: answering.map(|answering| msg_request::Answering {
            question: answering.question,
            answer: answering.answer.clone(),
        }),
        ..Default::default()
    };
    match answer_as_human(&group.daemon, Asked::Post(post)) {
        Ok(answer) => match answer.answer {
            Some(msg_answer::Answer::Posted(posted)) => {
                let reached: Vec<Reached> = posted.reached.iter().map(messaging::reached).collect();
                let called = answer.called.into_iter().collect();
                Sent::Posted {
                    seq: posted.seq,
                    said: posting::said(&reached, &called),
                    heard: posting::heard(&reached),
                }
            }
            // A daemon that keeps a post answers it as posted; one that says nothing more was
            // still done, and only whom it reached is unknown.
            _ => Sent::Posted {
                seq: 0,
                said: "kept; the daemon did not say whom it woke".to_string(),
                heard: true,
            },
        },
        Err(Refusal::Unanswered { detail, .. }) => match settled(group, to, text, after) {
            Some(seq) => {
                Sent::Posted { seq, said: posting::KEPT_UNANSWERED.to_string(), heard: true }
            }
            None => Sent::Unknown {
                text: text.to_string(),
                why: format!(
                    "{detail}. It may have been posted; it was not sent again, since a second \
                     send could post it twice. Open the conversation to see whether it landed."
                ),
            },
        },
        Err(refusal) => Sent::Refused { text: text.to_string(), why: refusal.detail().to_string() },
    }
}

/// The number of the human's entry that shows an unanswered post was kept, read once from the
/// group's log.
fn settled(group: &GroupKey, to: &[String], text: &str, after: u64) -> Option<u64> {
    let log = msg_request::Log { group: group.group.clone(), since: after, ..Default::default() };
    let answer = answer_as_human(&group.daemon, Asked::Log(log)).ok()?;
    let mut held = Log::default();
    held.followed(messaging::log_page(&answer, &group.group).entries);
    posting::settles(held.logged(), after, to, text)
}

/// How a request that posted answers the shell: whom it woke, the daemon's refusal, or that it
/// may have been kept.
pub(crate) fn response(sent: &Sent) -> Response {
    match sent {
        Sent::Posted { said, .. } => Response::noted(said.clone()),
        Sent::Refused { why, .. } => Response::failure(why.clone()),
        Sent::Unknown { why, .. } => Response::unanswered(why.clone()),
        Sent::Pending { .. } => Response::ok(),
    }
}

/// The outcome's name, for the run log.
pub(crate) fn outcome(sent: &Sent) -> &'static str {
    match sent {
        Sent::Pending { .. } => "pending",
        Sent::Posted { .. } => "posted",
        Sent::Refused { .. } => "refused",
        Sent::Unknown { .. } => "unknown",
    }
}
