//! The message service against `corpus/conformance/messaging.json`: each case is a script of
//! steps by named callers, and each step's result is rendered as one line, so a case reads as a
//! transcript of what the service said.

use std::cell::RefCell;
use std::collections::BTreeSet;

use conformance::{Conformance, fields};
use muster_msg::{
    Caller, Inbox, Liveness, Memory, Messaging, Notice, Participant, Posted, Presence, Reach,
    Refusal, What,
};
use serde_json::{Value, json};

/// Every inbox answers except those whose session has died.
#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
}

impl Presence for Sessions {
    fn alive(&self, participant: &Participant) -> bool {
        participant.inbox.as_ref().is_none_or(|inbox| !self.dead.borrow().contains(&inbox.socket))
    }
}

fn socket(session: &str) -> String {
    format!("/tmp/cc-socks/{session}.sock")
}

fn caller(step: &Value) -> Caller {
    let text = |key: &str| step.get(key).and_then(Value::as_str).map(str::to_string);
    Caller {
        as_name: text("as"),
        inbox: text("session").map(|session| Inbox {
            socket: socket(&session),
            inode: step.get("inode").and_then(Value::as_u64).unwrap_or(1),
        }),
        pane: None,
        directory: text("directory"),
    }
}

fn notice(notice: &Notice) -> String {
    format!(
        "{} #{}-{} x{} to-you:{} from {}",
        notice.group,
        notice.first,
        notice.last,
        notice.count,
        notice.to_you,
        notice.from.join(",")
    )
}

fn refused(refusal: &Refusal) -> String {
    let detail = match refusal {
        Refusal::BadName { name }
        | Refusal::NoSuchParticipant { name }
        | Refusal::NotAParticipant { name } => name.clone(),
        Refusal::NameInUse { name, inbox } => {
            format!("{name} at {}", inbox.as_deref().unwrap_or("no inbox"))
        }
        Refusal::NoSuchGroup { group } | Refusal::PairTooLong { group } => group.clone(),
        Refusal::GroupNameClash { group, existing } => format!("{group} {existing}"),
        Refusal::NotAMember { name, group } | Refusal::AddresseeNotInGroup { name, group } => {
            format!("{name} {group}")
        }
        Refusal::WhichGroup { candidates } => candidates.join(","),
        Refusal::Unread { group, count } => format!("{group} {count}"),
        Refusal::BodyTooLarge { bytes } => bytes.to_string(),
        Refusal::Store { error } => error.clone(),
        Refusal::AddressedSelf | Refusal::NoGroup | Refusal::EmptyBody => String::new(),
    };
    format!("refused {} {detail}", refusal.code()).trim_end().to_string()
}

fn entry(entry: &muster_msg::Entry) -> String {
    match &entry.what {
        What::Message { author, to, body } if to.is_empty() => {
            format!("#{} {author}: {body}", entry.seq)
        }
        What::Message { author, to, body } => {
            format!("#{} {author}->{}: {body}", entry.seq, to.join(","))
        }
        What::Created { by } => format!("#{} created by {by}", entry.seq),
        What::Joined { who } => format!("#{} {who} joined", entry.seq),
        What::Left { who } => format!("#{} {who} left", entry.seq),
    }
}

fn list(entries: &[muster_msg::Entry]) -> String {
    entries.iter().map(entry).collect::<Vec<_>>().join(" | ")
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// Delivers a post's wakes the way the daemon does - to every inbox that has not died, marking
/// the others gone - and says what the post did for each participant it was meant to wake.
fn delivered(service: &mut Messaging<Memory>, sessions: &Sessions, posted: &Posted) -> String {
    let mut failed = BTreeSet::new();
    for wake in &posted.wakes {
        let reached = !sessions.dead.borrow().contains(&wake.inbox.socket);
        service.delivered(&wake.name, reached).unwrap();
        if !reached {
            failed.insert(wake.name.clone());
        }
    }
    let mut parts = vec![format!("{} posted #{} to {}", posted.author, posted.seq, posted.group)];
    for (label, wanted) in [
        ("woke", Reach::Woken),
        ("already woken", Reach::AlreadyWoken),
        ("waiting", Reach::Waiting),
        ("gone", Reach::Gone),
    ] {
        let names: Vec<String> = posted
            .reached
            .iter()
            .filter(|(name, reach)| {
                let reach = if failed.contains(name) { Reach::Gone } else { *reach };
                reach == wanted
            })
            .map(|(name, _)| {
                let told = posted
                    .wakes
                    .iter()
                    .find(|wake| wake.name == *name && !failed.contains(name))
                    .map(|wake| &wake.notice);
                let answered = posted
                    .answered
                    .iter()
                    .find(|answered| answered.name == *name)
                    .map(|answered| &answered.notice);
                match told.or(answered) {
                    Some(told) => format!("{name} [{}]", notice(told)),
                    None => name.clone(),
                }
            })
            .collect();
        if !names.is_empty() {
            parts.push(format!("{label} {}", names.join(", ")));
        }
    }
    parts.join("; ")
}

/// Runs one step and says what came of it in one line.
fn step(service: &mut Messaging<Memory>, sessions: &Sessions, step: &Value, now: u64) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str);
    let who = caller(step);
    let action = text("do").unwrap_or_default();
    let result = match action {
        "join" => service.join(&who, text("name"), text("group"), sessions, now).map(|joined| {
            let mut line = format!("{} joined", joined.name);
            if let Some(group) = &joined.group {
                line = format!("{line} {group}");
            }
            if joined.group.is_none() {
                line = format!("{} registered", joined.name);
            }
            if joined.created {
                line.push_str(" (created)");
            }
            if joined.took_over {
                line.push_str(" (took over)");
            }
            line
        }),
        "leave" => service.leave(&who, text("group"), now).map(|left| {
            if left.stopped && left.groups.is_empty() {
                format!("{} stopped", left.name)
            } else if left.stopped {
                format!("{} stopped (left {})", left.name, left.groups.join(","))
            } else {
                format!("{} left {}", left.name, left.groups.join(","))
            }
        }),
        "post" => {
            let to = strings(step.get("to"));
            let body = text("body").unwrap_or_default();
            service
                .post(&who, text("group"), &to, body, sessions, now)
                .map(|posted| delivered(service, sessions, &posted))
        }
        "read" => service.read(&who, text("group"), sessions).map(|read| {
            let groups: Vec<String> = read
                .groups
                .iter()
                .filter(|(_, entries)| !entries.is_empty())
                .map(|(group, entries)| format!("{group}: {}", list(entries)))
                .collect();
            if groups.is_empty() {
                format!("{} read nothing", read.name)
            } else {
                format!("{} read {}", read.name, groups.join("; "))
            }
        }),
        "wait" => service.wait(&who, text("group"), sessions).map(|waited| match waited {
            muster_msg::Waited::Ready(notices) => {
                format!("ready {}", notices.iter().map(notice).collect::<Vec<_>>().join("; "))
            }
            muster_msg::Waited::Waiting { ticket, superseded: None } => format!("waiting {ticket}"),
            muster_msg::Waited::Waiting { ticket, superseded: Some(older) } => {
                format!("waiting {ticket}, ended {older}")
            }
        }),
        "who" => service.who(text("group"), sessions).map(|members| {
            members
                .iter()
                .map(|member| {
                    let liveness = match member.liveness {
                        Liveness::Alive => "alive",
                        Liveness::Gone => "gone",
                        Liveness::Human => "human",
                    };
                    format!("{} {liveness}", member.name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "log" => {
            let since = step.get("since").and_then(Value::as_u64).unwrap_or(0);
            service.log(text("group").unwrap_or_default(), since).map(|entries| list(&entries))
        }
        "dies" => {
            sessions.dead.borrow_mut().insert(socket(text("session").unwrap_or_default()));
            Ok("died".to_string())
        }
        other => panic!("messaging.json: a step does {other:?}, which the driver does not know"),
    };
    result.unwrap_or_else(|refusal| refused(&refusal))
}

#[test]
fn messaging_conformance() {
    let corpus = Conformance::load("messaging.json");
    let ran = corpus.run(|given| {
        let mut service = Messaging::new(Memory::default());
        let sessions = Sessions::default();
        let steps = given.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        let said: Vec<Value> = steps
            .iter()
            .enumerate()
            .map(|(at, each)| json!(step(&mut service, &sessions, each, at as u64 + 1)))
            .collect();
        Ok(fields([("said", Some(Value::Array(said)))]))
    });
    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}
