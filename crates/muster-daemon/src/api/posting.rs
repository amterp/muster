//! `msg.post` and `msg.ack`: a program posting and acknowledging as the human (MIP-10, section
//! 5). Each becomes the request `muster msg` sends and is handled as one, so a program's post is
//! held to the policy, the guard and the question it answers as any post is, reaches a group kept
//! on another machine the same way, and is carried to the human's home when that is elsewhere.
//! What is left here is turning the daemon's answer into one of the API's five outcomes.

use std::sync::Arc;

use muster_api as api;
use muster_core::posting;
use muster_daemon_proto as proto;
use muster_msg::HUMAN;
use prost::Message;

use super::{Caller, refused};
use crate::api::Cause;
use crate::messages::{self, Handled};
use crate::operations;
use crate::session::{Reply, Shared};
use proto::answer::Detail;
use proto::msg_answer::Answer as Said;
use proto::msg_request::Request as Asked;

pub(super) fn post(
    shared: &Arc<Shared>,
    caller: &Caller,
    correlation: String,
    asked: &api::NewPost,
) {
    let refuse = |why: &str| refused(correlation.clone(), api::Refusal::Invalid, why);
    let answer = if asked.operation.is_empty() {
        refuse("a post needs an operation id, which makes asking again safe; nothing was posted")
    } else if caller.client.is_empty() {
        refuse(
            "a post is shown with the name of the program that sent it, and this connection's \
             Hello gave none; nothing was posted",
        )
    } else {
        let operation = proto::msg_request::Operation {
            client: caller.client.clone(),
            id: asked.operation.clone(),
            asked: operations::fingerprint(&asked.encode_to_vec()),
        };
        let (body, to) = match &asked.answering {
            Some(answering) => answer_of(shared, asked, answering),
            None => (asked.body.clone(), asked.to.clone()),
        };
        let post = proto::msg_request::Post {
            group: (!asked.group.is_empty()).then(|| asked.group.clone()),
            to,
            body,
            urgent: asked.urgent,
            answers: asked.answers.clone(),
            answering: asked.answering.as_ref().map(|answering| proto::msg_request::Answering {
                question: answering.question,
                answer: answering.answer.clone(),
            }),
            operation: Some(operation),
        };
        let author = if asked.r#as.is_empty() { HUMAN.to_string() } else { asked.r#as.clone() };
        match handled(shared, caller, &correlation, author, Asked::Post(post), false) {
            Ok((reply, reached)) => outcome(correlation, reply, reached, |said| match said {
                Said::Posted(posted) => Some((
                    posted.already,
                    Some(api::answer::Detail::Posted(api::Posted {
                        group: posted.group.clone(),
                        number: posted.seq,
                    })),
                )),
                _ => None,
            }),
            // Carried there, the post would be posted by a daemon this one keeps no record of
            // sending it to, so a retry after the human's home moved could post it twice.
            Err(machine) => refused(
                correlation,
                api::Refusal::Unsupported,
                format!(
                    "the human is homed on {machine}, whose daemon posts as them; post through \
                     that daemon's API, where the post's operation id is kept. Nothing was posted"
                ),
            ),
        }
    };
    shared.api.answer(caller.connection, answer);
}

/// The body and addressees of a post answering a question, as `muster msg post --answer` makes
/// them: the answer first and the post's body after it as a note, and for whoever asked, unless
/// the post names whom it is for.
fn answer_of(
    shared: &Shared,
    asked: &api::NewPost,
    answering: &api::Answering,
) -> (String, Vec<String>) {
    let body = posting::answer_body(&answering.answer, &asked.body);
    if !asked.to.is_empty() {
        return (body, asked.to.clone());
    }
    let questioner = shared.messages().asker(&asked.group, answering.question);
    (body, questioner.into_iter().collect())
}

pub(super) fn ack(shared: &Arc<Shared>, caller: &Caller, correlation: String, asked: &api::Ack) {
    let ack =
        Asked::Ack(proto::msg_request::Ack { group: asked.group.clone(), through: asked.through });
    let handled = handled(shared, caller, &correlation, HUMAN.to_string(), ack, true);
    let Ok((reply, reached)) = handled else { unreachable!("an acknowledgement may be carried") };
    let answer = outcome(correlation, reply, reached, |said| match said {
        Said::Acked(done) => Some((!done.moved, None)),
        _ => None,
    });
    shared.api.answer(caller.connection, answer);
}

/// The daemon's answer to `asked`, made as `author`, and whether it reached the machine it had
/// to be carried to, when it had to be; or, where it would be carried and `may_carry` says not,
/// that machine, with nothing done.
fn handled(
    shared: &Arc<Shared>,
    caller: &Caller,
    correlation: &str,
    author: String,
    asked: Asked,
    may_carry: bool,
) -> Result<(Reply, bool), String> {
    let request = proto::MsgRequest {
        caller: Some(proto::msg_request::Caller {
            as_name: Some(author),
            ..proto::msg_request::Caller::default()
        }),
        request: Some(asked),
    };
    let cause = Cause { connection: caller.connection, correlation: correlation.to_string() };
    // The program waits for its answer either way, so nothing ends a carried request early but
    // the link's own patience.
    let hung_up = || false;
    match messages::handle_caused(shared, request, Some(&cause), &hung_up) {
        Handled::Reply(reply) => Ok((reply, true)),
        Handled::Carrying(carrying) if may_carry => Ok(carrying.run_reaching(shared, &hung_up)),
        Handled::Carrying(carrying) => Err(carrying.machine().to_string()),
    }
}

/// The API's answer for the daemon's `reply`. `done` reads a done reply's answer: whether
/// nothing needed doing, and the detail to answer with.
fn outcome(
    correlation: String,
    reply: Reply,
    reached: bool,
    done: impl FnOnce(&Said) -> Option<(bool, Option<api::answer::Detail>)>,
) -> api::Answer {
    let said = match reply.detail.as_deref() {
        Some(Detail::Msg(answer)) => Some(answer),
        _ => None,
    };
    let answer = |outcome: api::Outcome, reason: String| api::Answer {
        correlation: correlation.clone(),
        outcome: outcome.into(),
        reason,
        ..api::Answer::default()
    };
    if !reached {
        // Carried nowhere: the refusal made here says nothing of what the other machine holds.
        return answer(api::Outcome::NotSent, reply.reason);
    }
    if reply.outcome != proto::Outcome::Refused {
        let read = said.and_then(|said| said.answer.as_ref()).and_then(done);
        let Some((already, detail)) = read else {
            return answer(
                api::Outcome::Unknown,
                format!("the daemon answered with nothing this request expects: {}", reply.reason),
            );
        };
        let outcome = if already { api::Outcome::AlreadySo } else { api::Outcome::Done };
        return api::Answer { detail, ..answer(outcome, String::new()) };
    }
    let code = said.map_or("", |said| said.refusal.as_str());
    match refusal_of(code) {
        Ok(refusal) => refused(correlation, refusal, reply.reason),
        Err(outcome) => answer(outcome, reply.reason),
    }
}

/// The API's refusal for the daemon's refusal `code`, or the outcome it is instead: a request
/// that never left for the machine it had to go to was not sent, and one whose reply was lost
/// may have been done.
fn refusal_of(code: &str) -> Result<api::Refusal, api::Outcome> {
    Ok(match code {
        "unreachable" => return Err(api::Outcome::NotSent),
        "unanswered" => return Err(api::Outcome::Unknown),
        "no_such_group" | "no_such_participant" | "no_such_question" | "not_a_member" => {
            api::Refusal::NotFound
        }
        "already_answered" => api::Refusal::Answered,
        "operation_reused" => api::Refusal::OperationReused,
        // A replica waiting to be fetched again: nothing was sent, and it is soon over.
        "handing_over" | "unchecked" => api::Refusal::Unavailable,
        "urgent_unsupported" | "answers_unsupported" | "operations_unsupported" => {
            api::Refusal::Unsupported
        }
        "empty_body"
        | "body_too_large"
        | "too_many_answers"
        | "answer_too_long"
        | "bad_answer"
        | "answer_offers_answers"
        | "not_offered"
        | "bad_name"
        | "addressed_self"
        | "past_newest" => api::Refusal::Invalid,
        _ => api::Refusal::Unspecified,
    })
}
