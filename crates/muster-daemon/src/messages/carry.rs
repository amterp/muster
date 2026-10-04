//! The person at a shell on a machine whose human is homed on another (MIP-4, section 10). A
//! request the service refuses them for acting as the human, for changing a group kept where
//! the human is homed, or for naming a group this machine does not hold, is carried there over
//! the link and done as the human, and that machine's answer, in its names, is theirs.

use muster_daemon_proto as proto;
use muster_msg::{HumanHome, Messaging, Peer};
use proto::msg_request::Request as Asked;

use super::presence::Panes;
use super::store::Files;
use super::{policy_from, policy_of};
use crate::session::{Reply, Shared};

/// Where `asked`, just answered with `reply`, is to be carried: the human's home, when `reply`
/// refuses the person for acting as the human there, for changing a group kept there, or for
/// naming a group this machine has none of, which the home may keep.
pub(super) fn destination(
    shared: &Shared,
    caller: &muster_msg::Caller,
    asked: &Asked,
    reply: &Reply,
    panes: &Panes,
) -> Option<HumanHome> {
    let code = match reply.detail.as_deref() {
        Some(proto::answer::Detail::Msg(answer)) => answer.refusal.as_str(),
        _ => return None,
    };
    let messages = shared.messages();
    let home = messages.service.person_elsewhere(caller, panes)?;
    match code {
        "human_elsewhere" | "no_such_group" => Some(home),
        "kept_elsewhere" => {
            let kept = messages.service.home_of(group_of(asked)?)?;
            (kept == home.machine).then_some(home)
        }
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
        other => other,
    }
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
    }
}
