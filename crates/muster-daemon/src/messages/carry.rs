//! The person at a shell on a machine whose human is homed on another (MIP-4, section 10). A
//! request the service refuses them for acting as the human, for changing a group kept where
//! the human is homed, or for naming a group this machine does not hold, is carried there over
//! the link and done as the human, and that machine's answer, in its names, is theirs.
//!
//! The other way too: the person on the human's own machine deleting, pausing or resuming a
//! group kept on a linked machine has it carried to the group's home, which does it as the
//! human, and its answer is turned into this machine's names. An agent's delete, pause or resume
//! of such a group is carried the same way, and done there as that agent, as the home names it.

use muster_daemon_proto as proto;
use muster_msg::{HumanHome, Messaging, Peer};
use proto::msg_answer::Answer;
use proto::msg_request::Request as Asked;

use super::presence::Panes;
use super::store::Files;
use super::{policy_from, policy_of};
use crate::session::{Reply, Shared};

/// Where a person's refused request is carried.
pub(super) enum Destination {
    /// The human's home, which answers in its own names.
    HumanHome(HumanHome),
    /// The machine keeping the group the human here, or `agent` here, asked to change, whose
    /// answer is turned into this machine's names. `agent` is its name here.
    GroupHome { machine: String, agent: Option<String> },
}

/// Where `asked`, just answered with `reply`, is to be carried: the human's home, when `reply`
/// refuses the person for acting as the human there, for changing a group kept there, or for
/// naming a group this machine has none of, which the home may keep; or, for the human or an
/// agent here, the home of a group kept elsewhere that they asked to delete, pause or resume.
pub(super) fn destination(
    shared: &Shared,
    caller: &muster_msg::Caller,
    asked: &Asked,
    reply: &Reply,
    panes: &Panes,
) -> Option<Destination> {
    let code = match reply.detail.as_deref() {
        Some(proto::answer::Detail::Msg(answer)) => answer.refusal.as_str(),
        _ => return None,
    };
    let mut messages = shared.messages();
    let Some(home) = messages.service.person_elsewhere(caller, panes) else {
        let group = carried_to_its_home(asked)?;
        if code != "kept_elsewhere" {
            return None;
        }
        let machine = messages.service.home_of(group)?;
        let agent = if messages.service.is_person(caller, panes) {
            None
        } else {
            Some(messages.service.carried_as(caller, panes)?)
        };
        return Some(Destination::GroupHome { machine, agent });
    };
    match code {
        "human_elsewhere" | "no_such_group" => Some(Destination::HumanHome(home)),
        "kept_elsewhere" => {
            let kept = messages.service.home_of(group_of(asked)?)?;
            (kept == home.machine).then_some(Destination::HumanHome(home))
        }
        _ => None,
    }
}

/// A refusal for carrying `asked` to `machine`, whose daemon would drop part of it without a word
/// or refuse it without saying why: a program's post loses its operation id there, so a retry
/// would post twice, and an answer in words is refused as not offering the empty answer.
pub(super) fn unkept_at(
    shared: &Shared,
    machine: &str,
    asked: &Asked,
    agent: Option<&str>,
) -> Option<Reply> {
    let speaks = shared.peers.to(machine)?.speaks;
    let (code, words) = unkept(asked, speaks, machine)
        .or_else(|| agent.and_then(|agent| uncarried_agent(agent, speaks, machine)))?;
    Some(super::refused_as("", code, &words))
}

/// Why `agent`'s request is not carried to `machine`, when its daemon would do it as the human.
fn uncarried_agent(
    agent: &str,
    speaks: proto::Version,
    machine: &str,
) -> Option<(&'static str, String)> {
    (speaks.minor < proto::version::AGENTS_CARRIED).then(|| {
        let words = format!(
            "this group is kept on {machine}, whose muster-daemon speaks protocol {speaks}, which \
             would do this as the person rather than as {agent}. Nothing was sent; update Muster \
             there, or run it in a pane on {machine}"
        );
        ("kept_elsewhere", words)
    })
}

fn unkept(asked: &Asked, speaks: proto::Version, machine: &str) -> Option<(&'static str, String)> {
    let operation = matches!(asked, Asked::Post(post) if post.operation.is_some())
        || matches!(asked, Asked::Ack(_));
    if operation && speaks.minor < proto::version::OPERATIONS {
        let words = format!(
            "this is done on {machine}, whose muster-daemon speaks protocol {speaks}, which \
             cannot acknowledge messages or keep a program's operation id, without which a retry \
             could post twice. Nothing was sent; update Muster there"
        );
        return Some(("operations_unsupported", words));
    }
    let in_words = matches!(asked, Asked::Post(post) if super::answers_in_words(post));
    if in_words && speaks.minor < proto::version::ANSWERS_IN_WORDS {
        let words = format!(
            "this is done on {machine}, whose muster-daemon speaks protocol {speaks}, which \
             cannot keep an answer in words, with none of the question's answers picked. Nothing \
             was sent; update Muster there, or pick one of its answers"
        );
        return Some(("in_words_unsupported", words));
    }
    let recommends = matches!(asked, Asked::Post(post) if post.recommended.is_some());
    (recommends && speaks.minor < proto::version::RECOMMENDS_ANSWERS).then(|| {
        let words = format!(
            "this is done on {machine}, whose muster-daemon speaks protocol {speaks}, which \
             cannot keep the answer a question recommends. Nothing was sent; update Muster \
             there, or post without --recommend"
        );
        ("recommended_unsupported", words)
    })
}

/// Whether `asked` may be carried if this machine refuses it, which is what it is kept for.
pub(super) fn may_carry(
    service: &Messaging<Files>,
    caller: &muster_msg::Caller,
    asked: &Asked,
    panes: &Panes,
) -> bool {
    service.person_elsewhere(caller, panes).is_some() || carried_to_its_home(asked).is_some()
}

/// The group a request the human here may have carried to the group's home changes. Only the
/// verbs that name nothing but the group: the names in a policy or a member list are turned
/// for the other direction, where this machine is not the human's home.
pub(super) fn carried_to_its_home(asked: &Asked) -> Option<&str> {
    match asked {
        Asked::GroupDelete(delete) => Some(&delete.group),
        Asked::Pause(pause) => Some(&pause.group),
        Asked::Resume(resume) => Some(&resume.group),
        _ => None,
    }
}

/// The group a request changes, for those that change one.
fn group_of(asked: &Asked) -> Option<&str> {
    match asked {
        Asked::GroupSet(set) => Some(&set.group),
        Asked::GroupMembers(members) => Some(&members.group),
        Asked::Pause(pause) => Some(&pause.group),
        Asked::Resume(resume) => Some(&resume.group),
        Asked::GroupDelete(delete) => Some(&delete.group),
        Asked::Post(post) => post.group.as_deref(),
        _ => None,
    }
}

/// `asked` with its names as this machine writes them, for the human's home to turn into its
/// own.
pub(super) fn outward(service: &Messaging<Files>, asked: Asked, panes: &Panes) -> Asked {
    let group = |group: String| service.carrying_group(&group);
    let name = |name: String| service.carrying_name(&name, panes);
    let policy = |policy| policy_of(&service.carrying_policy(policy_from(policy), panes));
    turned(asked, &group, &name, &policy)
}

/// A request `peer`'s machine carried here, with its names turned into this machine's.
pub(super) fn inward(peer: &Peer, asked: Asked) -> Asked {
    let name = |name: String| peer.inward(&name);
    let policy = |policy| policy_of(&peer.policy(policy_from(policy)));
    turned(asked, &name, &name, &policy)
}

/// `asked` with every group, participant and policy in it passed through the functions given.
/// A join's name is the one the caller takes, not one it names, so it is left as it is.
fn turned(
    asked: Asked,
    group: &dyn Fn(String) -> String,
    name: &dyn Fn(String) -> String,
    policy: &dyn Fn(proto::msg_request::Policy) -> proto::msg_request::Policy,
) -> Asked {
    let names = |names: Vec<String>| names.into_iter().map(name).collect();
    match asked {
        Asked::Join(mut join) => {
            join.group = join.group.map(group);
            Asked::Join(join)
        }
        Asked::Leave(mut leave) => {
            leave.group = leave.group.map(group);
            Asked::Leave(leave)
        }
        Asked::Post(mut post) => {
            post.group = post.group.map(group);
            post.to = names(post.to);
            Asked::Post(post)
        }
        Asked::Read(mut read) => {
            read.group = read.group.map(group);
            Asked::Read(read)
        }
        Asked::Wait(mut wait) => {
            wait.group = wait.group.map(group);
            Asked::Wait(wait)
        }
        Asked::GroupNew(mut new) => {
            new.group = group(new.group);
            new.policy = new.policy.map(policy);
            Asked::GroupNew(new)
        }
        Asked::GroupSet(mut set) => {
            set.group = group(set.group);
            set.policy = set.policy.map(policy);
            Asked::GroupSet(set)
        }
        Asked::GroupMembers(mut members) => {
            members.group = group(members.group);
            members.add = names(members.add);
            members.remove = names(members.remove);
            Asked::GroupMembers(members)
        }
        Asked::Pause(mut pause) => {
            pause.group = group(pause.group);
            Asked::Pause(pause)
        }
        Asked::Resume(mut resume) => {
            resume.group = group(resume.group);
            Asked::Resume(resume)
        }
        Asked::GroupDelete(mut delete) => {
            delete.group = group(delete.group);
            Asked::GroupDelete(delete)
        }
        Asked::Ack(mut ack) => {
            ack.group = group(ack.group);
            Asked::Ack(ack)
        }
        other => other,
    }
}

/// A group's home's answer to a request carried there, in this machine's names: `peer` is
/// that home, as this machine knows it.
pub(super) fn named_here(peer: &Peer, mut reply: Reply) -> Reply {
    let Some(proto::answer::Detail::Msg(answer)) = reply.detail.as_deref_mut() else {
        return reply;
    };
    let name = |name: &mut String| *name = peer.inward(name);
    if !answer.caller.is_empty() {
        name(&mut answer.caller);
    }
    answer.called = std::mem::take(&mut answer.called)
        .into_iter()
        .map(|(id, callsign)| (peer.inward(&id), peer.inward(&callsign)))
        .collect();
    match &mut answer.answer {
        Some(Answer::Changed(changed)) => {
            name(&mut changed.group);
            changed.added.iter_mut().chain(&mut changed.removed).for_each(name);
        }
        Some(Answer::Deleted(deleted)) => {
            name(&mut deleted.group);
            deleted.let_go.iter_mut().for_each(name);
        }
        Some(Answer::Resumed(resumed)) => {
            name(&mut resumed.group);
            resumed.reached.iter_mut().for_each(|reached| name(&mut reached.name));
        }
        _ => {}
    }
    reply
}

/// What a carried request came to, for the link.
pub(super) fn reply_to(reply: Reply) -> proto::peer_reply::Carried {
    let answer = match reply.detail.map(|detail| *detail) {
        Some(proto::answer::Detail::Msg(answer)) => Some(answer),
        _ => None,
    };
    proto::peer_reply::Carried {
        refused: reply.outcome == proto::Outcome::Refused,
        reason: reply.reason,
        answer,
    }
}

/// A carried request's answer, as this machine gives it to the caller that made it.
pub(super) fn reply_from(carried: proto::peer_reply::Carried) -> Reply {
    let mut reply = if carried.refused { Reply::refused(carried.reason) } else { Reply::done() };
    reply.detail = carried.answer.map(|answer| Box::new(proto::answer::Detail::Msg(answer)));
    reply
}

/// What a request is, for a log line.
pub(super) fn verb(asked: &Asked) -> &'static str {
    match asked {
        Asked::Join(_) => "join",
        Asked::Leave(_) => "leave",
        Asked::Who(_) => "who",
        Asked::Post(_) => "post",
        Asked::Read(_) => "read",
        Asked::Log(_) => "log",
        Asked::Wait(_) => "wait",
        Asked::Groups(_) => "groups",
        Asked::GroupNew(_) => "group_new",
        Asked::GroupSet(_) => "group_set",
        Asked::GroupMembers(_) => "group_members",
        Asked::Pause(_) => "pause",
        Asked::Resume(_) => "resume",
        Asked::GroupDelete(_) => "group_delete",
        Asked::Peer(_) => "peer",
        Asked::Ack(_) => "ack",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(operation: Option<proto::msg_request::Operation>) -> Asked {
        Asked::Post(proto::msg_request::Post { operation, ..Default::default() })
    }

    #[test]
    fn an_answer_in_words_is_not_carried_to_a_daemon_that_would_refuse_it_unexplained() {
        let in_words = |answer: &str| {
            Asked::Post(proto::msg_request::Post {
                answering: Some(proto::msg_request::Answering {
                    question: 4,
                    answer: answer.to_string(),
                }),
                ..Default::default()
            })
        };
        let older = proto::Version { major: 1, minor: proto::version::ANSWERS_IN_WORDS - 1 };
        let current = proto::Version { major: 1, minor: proto::version::ANSWERS_IN_WORDS };
        let refused = unkept(&in_words(""), older, "lap").map(|(code, _)| code);
        assert_eq!(refused, Some("in_words_unsupported"));
        assert!(unkept(&in_words(""), current, "lap").is_none());
        assert!(unkept(&in_words("Yes"), older, "lap").is_none());
        let recommending = Asked::Post(proto::msg_request::Post {
            answers: vec!["Yes".to_string()],
            recommended: Some("Yes".to_string()),
            ..Default::default()
        });
        let refused = unkept(&recommending, older, "lap").map(|(code, _)| code);
        assert_eq!(refused, Some("recommended_unsupported"));
        assert!(unkept(&recommending, current, "lap").is_none());
    }

    #[test]
    fn a_programs_post_is_not_carried_to_a_daemon_that_would_drop_its_operation_id() {
        let operation =
            proto::msg_request::Operation { client: "foreman".into(), ..Default::default() };
        let older = proto::Version { major: 1, minor: proto::version::OPERATIONS - 1 };
        let current = proto::Version { major: 1, minor: proto::version::OPERATIONS };
        assert!(unkept(&post(Some(operation.clone())), older, "lap").is_some());
        assert!(unkept(&post(Some(operation)), current, "lap").is_none());
        assert!(unkept(&post(None), older, "lap").is_none());
    }
}
