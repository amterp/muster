//! Between muster-msg's calls across machines and the frames one daemon sends another.

use muster_daemon_proto as proto;
use muster_msg::{
    Action, Call, Caught, Change, Entry, Liveness, Member, Reach, Refusal, Reply, What,
};
use proto::msg_answer;
use proto::peer_call::{self, Call as Called};
use proto::peer_reply::{self, Reply as Replied};

use super::{activity_of, entry_of, member_of, policy_from, policy_of, reach_of};

pub(super) fn call_to(call: &Call) -> Called {
    match call.clone() {
        Call::Find { group } => Called::Find(peer_call::Find { group }),
        Call::Join { name, group, head } => Called::Join(peer_call::Join { name, group, head }),
        Call::Leave { name, group, head } => Called::Leave(peer_call::Leave { name, group, head }),
        Call::Post { author, group, to, body, urgent, cursor, head } => {
            Called::Post(peer_call::Post { author, group, to, body, cursor, head, urgent })
        }
        Call::Since { group, after } => Called::Since(peer_call::Since { group, after }),
        Call::Who { group } => Called::Who(peer_call::Who { group }),
        Call::Whom { name } => Called::Whom(peer_call::Whom { name }),
    }
}

/// The call, or nothing for a replicate or a carried request, which the service does not
/// answer as calls.
pub(super) fn call_from(called: Called) -> Option<Call> {
    Some(match called {
        Called::Find(find) => Call::Find { group: find.group },
        Called::Join(join) => Call::Join { name: join.name, group: join.group, head: join.head },
        Called::Leave(leave) => {
            Call::Leave { name: leave.name, group: leave.group, head: leave.head }
        }
        Called::Post(post) => Call::Post {
            author: post.author,
            group: post.group,
            to: post.to,
            body: post.body,
            urgent: post.urgent,
            cursor: post.cursor,
            head: post.head,
        },
        Called::Since(since) => Call::Since { group: since.group, after: since.after },
        Called::Who(who) => Call::Who { group: who.group },
        Called::Whom(whom) => Call::Whom { name: whom.name },
        Called::Replicate(_) | Called::Carried(_) | Called::Forget(_) => return None,
    })
}

pub(super) fn caught_to(caught: &Caught) -> proto::Caught {
    proto::Caught {
        group: caught.group.clone(),
        entries: caught.entries.iter().map(entry_of).collect(),
        policy: Some(policy_of(&caught.policy)),
        more: caught.more,
    }
}

pub(super) fn caught_from(caught: proto::Caught) -> Caught {
    Caught {
        group: caught.group,
        entries: caught.entries.into_iter().filter_map(entry_from).collect(),
        policy: caught.policy.map(policy_from).unwrap_or_default(),
        more: caught.more,
    }
}

fn entry_from(entry: msg_answer::Entry) -> Option<Entry> {
    use msg_answer::entry::What as Said;
    let what = match entry.what? {
        Said::Message(message) => What::Message {
            author: message.author,
            to: message.to,
            body: message.body,
            urgent: message.urgent,
        },
        Said::Created(by) => What::Created { by },
        Said::Joined(who) => What::Joined { who },
        Said::Left(who) => What::Left { who },
        Said::Changed(changed) => {
            let change = match changed.change() {
                msg_answer::entry::Change::SetPolicy => Change::SetPolicy,
                msg_answer::entry::Change::Paused => Change::Paused,
                msg_answer::entry::Change::Resumed => Change::Resumed,
                msg_answer::entry::Change::Unspecified => return None,
            };
            What::Changed { by: changed.by, change }
        }
    };
    Some(Entry { seq: entry.seq, at_ms: entry.at_ms, what })
}

pub(super) fn reached_to(reached: &[(String, Reach)]) -> Vec<msg_answer::Reached> {
    reached
        .iter()
        .map(|(name, reach)| msg_answer::Reached {
            name: name.clone(),
            reach: reach_of(*reach).into(),
            activity: activity_of(None).into(),
            until: msg_answer::Until::Unspecified.into(),
        })
        .collect()
}

pub(super) fn reached_from(reached: Vec<msg_answer::Reached>) -> Vec<(String, Reach)> {
    reached
        .into_iter()
        .map(|reached| {
            let reach = match reached.reach() {
                msg_answer::Reach::Woken => Reach::Woken,
                msg_answer::Reach::AlreadyWoken => Reach::AlreadyWoken,
                msg_answer::Reach::Waiting => Reach::Waiting,
                msg_answer::Reach::Deferred => Reach::Deferred,
                msg_answer::Reach::NoAgent => Reach::NoAgent,
                msg_answer::Reach::NoDoorbell => Reach::NoDoorbell,
                msg_answer::Reach::Gone | msg_answer::Reach::Unspecified => Reach::Gone,
                msg_answer::Reach::Unreachable => Reach::Unreachable,
                msg_answer::Reach::Paused => Reach::Paused,
            };
            (reached.name, reach)
        })
        .collect()
}

fn member_from(member: msg_answer::Member) -> Member {
    let liveness = match member.liveness() {
        msg_answer::Liveness::Alive => Liveness::Alive,
        msg_answer::Liveness::Human => Liveness::Human,
        msg_answer::Liveness::Unreachable => Liveness::Unreachable,
        msg_answer::Liveness::Gone | msg_answer::Liveness::Unspecified => Liveness::Gone,
    };
    let activity = match member.activity() {
        msg_answer::Activity::Unspecified => None,
        msg_answer::Activity::Working => Some(muster_msg::Activity::Working),
        msg_answer::Activity::Blocked => Some(muster_msg::Activity::Blocked),
        msg_answer::Activity::Idle => Some(muster_msg::Activity::Idle),
        msg_answer::Activity::Waiting => Some(muster_msg::Activity::Waiting),
    };
    Member {
        name: member.name,
        liveness,
        activity,
        groups: member.groups.iter().map(|group| plain(group)).collect(),
        inbox: member.inbox.as_deref().map(plain),
        pane: member.pane.as_deref().map(plain),
    }
}

/// Text another machine sent that is printed here but names nothing, without anything a
/// terminal would take as a command.
fn plain(text: &str) -> String {
    text.chars().filter(|character| !character.is_control()).collect()
}

pub(super) fn reply_to(reply: Reply) -> Replied {
    match reply {
        Reply::Found(found) => Replied::Found(found),
        Reply::Joined { seq, caught } => {
            Replied::Joined(peer_reply::Joined { seq, caught: Some(caught_to(&caught)) })
        }
        Reply::Left { caught } => Replied::Left(caught_to(&caught)),
        Reply::Posted { seq, reached, caught } => Replied::Posted(peer_reply::Posted {
            seq,
            reached: reached_to(&reached),
            caught: Some(caught_to(&caught)),
        }),
        Reply::Caught(caught) => Replied::Caught(caught_to(&caught)),
        Reply::Members(members) => Replied::Members(msg_answer::Members {
            members: members.into_iter().map(member_of).collect(),
        }),
        Reply::Named(name) => Replied::Named(peer_reply::Named { name }),
        Reply::Refused { refusal, caught } => {
            let mut refused = refusal_to(&refusal);
            refused.caught = caught.as_ref().map(caught_to);
            Replied::Refused(refused)
        }
    }
}

/// The reply, or nothing for an applied or a carried, which answer a replicate or a carried
/// request rather than a call.
pub(super) fn reply_from(replied: Replied) -> Option<Reply> {
    let caught = |caught: Option<proto::Caught>| caught_from(caught.unwrap_or_default());
    Some(match replied {
        Replied::Found(found) => Reply::Found(found),
        Replied::Joined(joined) => Reply::Joined { seq: joined.seq, caught: caught(joined.caught) },
        Replied::Left(left) => Reply::Left { caught: caught_from(left) },
        Replied::Posted(posted) => Reply::Posted {
            seq: posted.seq,
            reached: reached_from(posted.reached),
            caught: caught(posted.caught),
        },
        Replied::Caught(got) => Reply::Caught(caught_from(got)),
        Replied::Members(members) => {
            Reply::Members(members.members.into_iter().map(member_from).collect())
        }
        Replied::Refused(refused) => {
            let caught = refused.caught.clone().map(caught_from);
            Reply::Refused { refusal: refusal_from(refused), caught }
        }
        Replied::Named(named) => Reply::Named(named.name),
        Replied::Applied(_) | Replied::Carried(_) => return None,
    })
}

pub(super) fn refusal_to(refusal: &Refusal) -> peer_reply::Refused {
    let mut refused = peer_reply::Refused {
        code: refusal.code().to_string(),
        words: super::words(refusal),
        ..peer_reply::Refused::default()
    };
    match refusal.clone() {
        Refusal::BadName { name } | Refusal::NoSuchParticipant { name } => refused.name = name,
        Refusal::NameInUse { name, inbox } => {
            refused.name = name;
            refused.existing = inbox.unwrap_or_default();
        }
        Refusal::NoSuchGroup { group }
        | Refusal::PairTooLong { group }
        | Refusal::GroupExists { group } => refused.group = group,
        Refusal::GroupNameClash { group, existing } => {
            refused.group = group;
            refused.existing = existing;
        }
        Refusal::WhichParticipant { name, candidates } => {
            refused.name = name;
            refused.candidates = candidates;
        }
        Refusal::Unreachable { group, machine }
        | Refusal::Unanswered { group, machine }
        | Refusal::KeptElsewhere { group, machine } => {
            refused.group = group;
            refused.machine = machine;
        }
        Refusal::HumanElsewhere { machine, calls_us } => {
            refused.machine = machine;
            refused.name = calls_us;
        }
        Refusal::NotAllowed { addressee, group, allowed } => {
            refused.name = addressee;
            refused.group = group;
            refused.candidates = allowed;
        }
        Refusal::NotUrgent { group, urgent } => {
            refused.group = group;
            refused.candidates = urgent;
        }
        Refusal::NotPermitted { name, group, action, permitted } => {
            refused.name = name;
            refused.group = group;
            refused.action = action_to(action).to_string();
            refused.candidates = permitted;
        }
        Refusal::NotAParticipant { name } => refused.name = name.unwrap_or_default(),
        Refusal::NotAMember { name, group } | Refusal::AddresseeNotInGroup { name, group } => {
            refused.name = name;
            refused.group = group;
        }
        Refusal::WhichGroup { candidates } => refused.candidates = candidates,
        Refusal::Unchecked { group, machines } => {
            refused.group = group;
            refused.candidates = machines;
        }
        Refusal::Unread { group, count } => {
            refused.group = group;
            refused.count = count;
        }
        Refusal::BodyTooLarge { bytes } => refused.count = bytes as u64,
        Refusal::Store { error } => refused.existing = error,
        Refusal::AddressedSelf | Refusal::NoGroup | Refusal::EmptyBody => {}
    }
    refused
}

/// The refusal a code names, with the fields it carries. A code this build does not know - a
/// newer daemon's - keeps the other daemon's own words. Every field is printed here, so none
/// keeps what a terminal would take as a command.
fn refusal_from(refused: peer_reply::Refused) -> Refusal {
    let peer_reply::Refused {
        code,
        words,
        name,
        group,
        machine,
        count,
        existing,
        candidates,
        action,
        ..
    } = refused;
    let (words, name, group, machine) =
        (plain(&words), plain(&name), plain(&group), plain(&machine));
    let (existing, candidates) =
        (plain(&existing), candidates.iter().map(|name| plain(name)).collect());
    let optional = |text: String| (!text.is_empty()).then_some(text);
    match code.as_str() {
        "bad_name" => Refusal::BadName { name },
        "name_in_use" => Refusal::NameInUse { name, inbox: optional(existing) },
        "no_such_group" => Refusal::NoSuchGroup { group },
        "group_name_clash" => Refusal::GroupNameClash { group, existing },
        "pair_too_long" => Refusal::PairTooLong { group },
        "no_such_participant" => Refusal::NoSuchParticipant { name },
        "which_participant" => Refusal::WhichParticipant { name, candidates },
        "unreachable" => Refusal::Unreachable { group, machine },
        "unanswered" => Refusal::Unanswered { group, machine },
        "kept_elsewhere" => Refusal::KeptElsewhere { group, machine },
        "human_elsewhere" => Refusal::HumanElsewhere { machine, calls_us: name },
        "not_allowed" => Refusal::NotAllowed { addressee: name, group, allowed: candidates },
        "not_urgent" => Refusal::NotUrgent { group, urgent: candidates },
        "not_permitted" if let Some(action) = action_from(&action) => {
            Refusal::NotPermitted { name, group, action, permitted: candidates }
        }
        "group_exists" => Refusal::GroupExists { group },
        "not_a_participant" => Refusal::NotAParticipant { name: optional(name) },
        "not_a_member" => Refusal::NotAMember { name, group },
        "addressee_not_in_group" => Refusal::AddresseeNotInGroup { name, group },
        "addressed_self" => Refusal::AddressedSelf,
        "no_group" => Refusal::NoGroup,
        "which_group" => Refusal::WhichGroup { candidates },
        "unchecked" => Refusal::Unchecked { group, machines: candidates },
        "unread" => Refusal::Unread { group, count },
        "empty_body" => Refusal::EmptyBody,
        "body_too_large" => {
            Refusal::BodyTooLarge { bytes: usize::try_from(count).unwrap_or(usize::MAX) }
        }
        "store" => Refusal::Store { error: existing },
        _ => Refusal::Store { error: format!("the other machine refused this: {words}") },
    }
}

fn action_to(action: Action) -> &'static str {
    match action {
        Action::Join => "join",
        Action::Leave => "leave",
        Action::Add => "add",
        Action::Remove => "remove",
        Action::SetPolicy => "set_policy",
        Action::Pause => "pause",
        Action::Resume => "resume",
        Action::Delete => "delete",
    }
}

fn action_from(action: &str) -> Option<Action> {
    Some(match action {
        "join" => Action::Join,
        "leave" => Action::Leave,
        "add" => Action::Add,
        "remove" => Action::Remove,
        "set_policy" => Action::SetPolicy,
        "pause" => Action::Pause,
        "resume" => Action::Resume,
        "delete" => Action::Delete,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal crosses the link as fields, and comes back as the same refusal, so the asker
    /// words it with its own names.
    #[test]
    fn every_refusal_crosses_the_link_intact() {
        let refusals = [
            Refusal::BadName { name: "a b".into() },
            Refusal::NameInUse { name: "a".into(), inbox: Some("/tmp/a.sock".into()) },
            Refusal::NoSuchGroup { group: "g".into() },
            Refusal::GroupNameClash { group: "G".into(), existing: "g".into() },
            Refusal::PairTooLong { group: "a+b".into() },
            Refusal::NoSuchParticipant { name: "a".into() },
            Refusal::WhichParticipant { name: "a".into(), candidates: vec!["a@x".into()] },
            Refusal::Unreachable { group: "g@x".into(), machine: "x".into() },
            Refusal::Unanswered { group: "g@x".into(), machine: "x".into() },
            Refusal::KeptElsewhere { group: "g@x".into(), machine: "x".into() },
            Refusal::HumanElsewhere { machine: "x".into(), calls_us: "y".into() },
            Refusal::NotAllowed {
                addressee: "b".into(),
                group: "g".into(),
                allowed: vec!["director".into(), "@human".into()],
            },
            Refusal::NotUrgent { group: "g".into(), urgent: vec!["director".into()] },
            Refusal::NotPermitted {
                name: "b".into(),
                group: "g".into(),
                action: Action::SetPolicy,
                permitted: vec!["director".into()],
            },
            Refusal::GroupExists { group: "g".into() },
            Refusal::NotAParticipant { name: Some("a".into()) },
            Refusal::NotAMember { name: "a".into(), group: "g".into() },
            Refusal::AddresseeNotInGroup { name: "a".into(), group: "g".into() },
            Refusal::AddressedSelf,
            Refusal::NoGroup,
            Refusal::WhichGroup { candidates: vec!["g".into(), "h".into()] },
            Refusal::Unread { group: "g".into(), count: 2 },
            Refusal::EmptyBody,
            Refusal::BodyTooLarge { bytes: 9 },
            Refusal::Store { error: "full".into() },
        ];
        for refusal in refusals {
            assert_eq!(refusal_from(refusal_to(&refusal)), refusal);
        }
    }

    #[test]
    fn a_caught_group_crosses_the_link_intact() {
        let caught = Caught {
            group: "review".into(),
            policy: muster_msg::Policy::default(),
            more: true,
            entries: vec![
                Entry {
                    seq: 4,
                    at_ms: 9,
                    what: What::Message {
                        author: "a".into(),
                        to: vec!["b".into()],
                        body: "x".into(),
                        urgent: true,
                    },
                },
                Entry {
                    seq: 5,
                    at_ms: 10,
                    what: What::Changed { by: "a".into(), change: Change::Paused },
                },
            ],
        };
        assert_eq!(caught_from(caught_to(&caught)), caught);
    }
}
