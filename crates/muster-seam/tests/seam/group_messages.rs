//! What the messaging tests ask a real daemon, about one group `g`: the human, `a` and `b` in it,
//! posts by its agents, and what its log holds.

use muster_daemon_proto as daemon_proto;
use muster_daemon_proto::msg_answer::{self, Answer, entry::What};
use muster_daemon_proto::msg_request::{self, Request as Asked};
use muster_daemon_proto::request::Service;
use muster_harness::Control;
use muster_harness::requests::{expect, snapshot};

/// The human, `a` and `b` in group `g`.
pub(crate) fn group_of_three(control: &mut Control) {
    for (caller, name) in [(the_human(), "@human"), (named("a"), "a"), (named("b"), "b")] {
        let join = Asked::Join(msg_request::Join {
            name: Some(name.to_string()),
            group: Some("g".to_string()),
            pull: false,
        });
        expect(control, msg(&caller, join), daemon_proto::Outcome::Done);
    }
}

pub(crate) fn the_human() -> msg_request::Caller {
    msg_request::Caller::default()
}

pub(crate) fn named(name: &str) -> msg_request::Caller {
    msg_request::Caller { as_name: Some(name.to_string()), ..msg_request::Caller::default() }
}

/// The agent in `pane`, under whatever name it joined by, or its pane's when it joined by none.
pub(crate) fn in_pane(pane: &str) -> msg_request::Caller {
    msg_request::Caller { pane: Some(pane.to_string()), ..msg_request::Caller::default() }
}

pub(crate) fn msg(caller: &msg_request::Caller, asked: Asked) -> Service {
    Service::Msg(daemon_proto::MsgRequest { caller: Some(caller.clone()), request: Some(asked) })
}

/// Posts as `from` to `to` in `g`, having read what waits there first as an agent does, and
/// returns the message's number.
pub(crate) fn post(control: &mut Control, from: &str, to: &str, body: &str) -> u64 {
    let posted = posted(control, &named(from), to, body, &[]);
    match posted.answer {
        Some(Answer::Posted(posted)) => posted.seq,
        other => panic!("a post answered with {other:?}"),
    }
}

/// Posts as `from` to `to` in `g`, offering `answers`, having read what waits there first, and
/// returns what the daemon answered.
pub(crate) fn posted(
    control: &mut Control,
    from: &msg_request::Caller,
    to: &str,
    body: &str,
    answers: &[&str],
) -> daemon_proto::MsgAnswer {
    let read = Asked::Read(msg_request::Read { group: Some("g".to_string()) });
    control.ask(msg(from, read));
    let asked = Asked::Post(msg_request::Post {
        group: Some("g".to_string()),
        to: vec![to.to_string()],
        body: body.to_string(),
        answers: answers.iter().map(ToString::to_string).collect(),
        ..Default::default()
    });
    let posted = control.ask(msg(from, asked));
    match posted.answer.detail {
        Some(daemon_proto::answer::Detail::Msg(answer)) => answer,
        other => panic!("a post answered with {other:?}: {}", posted.answer.reason),
    }
}

/// How many messages wait for the human in `g`, as the daemon says.
pub(crate) fn unread(control: &mut Control) -> u64 {
    snapshot(control).human.iter().find(|notice| notice.group == "g").map_or(0, |n| n.count)
}

/// Every entry in `g`'s log, read by `b`.
pub(crate) fn log(control: &mut Control) -> Vec<msg_answer::Entry> {
    let log = Asked::Log(msg_request::Log { group: "g".to_string(), ..Default::default() });
    let logged = expect(control, msg(&named("b"), log), daemon_proto::Outcome::Done);
    let Some(daemon_proto::answer::Detail::Msg(daemon_proto::MsgAnswer {
        answer: Some(Answer::Entries(entries)),
        ..
    })) = logged.answer.detail
    else {
        panic!("a log answered {:?}", logged.answer);
    };
    entries.groups.into_iter().flat_map(|group| group.entries).collect()
}

/// Every message in `g`: author, recipients, body.
pub(crate) fn entries(control: &mut Control) -> Vec<(String, Vec<String>, String)> {
    log(control)
        .into_iter()
        .filter_map(|entry| match entry.what {
            Some(What::Message(message)) => Some((message.author, message.to, message.body)),
            _ => None,
        })
        .collect()
}
