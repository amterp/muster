//! The message service against `corpus/conformance/messaging.json`: each case is a script of
//! steps by named callers, and each step's result is rendered as one line, so a case reads as a
//! transcript of what the service said.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use conformance::{Conformance, fields};
use muster_msg::{
    Activity, Caller, Inbox, Liveness, Memory, Messaging, Notice, Participant, Posted, Presence,
    Reach, Refusal, Ringable, Via, What,
};
use serde_json::{Value, json};

/// Every inbox answers except those whose session has died, and a pane has an agent in it while
/// a step has said so. A pane may also be open with no agent in it, new or not.
#[derive(Default)]
struct Sessions {
    dead: RefCell<BTreeSet<String>>,
    agents: RefCell<BTreeMap<String, Activity>>,
    /// Agents whose prompt the host cannot read.
    unreadable: RefCell<BTreeSet<String>>,
    /// Open panes with no agent, and whether each is new.
    open: RefCell<BTreeMap<String, bool>>,
    /// Whether a window is attending, which wakes the human.
    attended: Cell<bool>,
}

impl Sessions {
    fn answers(&self, inbox: &Inbox) -> bool {
        !self.dead.borrow().contains(&inbox.socket)
    }
}

impl Presence for Sessions {
    /// An agent in a pane is there while its pane has an agent, whatever its inbox says, as in
    /// the daemon: the pane is what the daemon watches.
    fn alive(&self, participant: &Participant) -> bool {
        match (&participant.pane, &participant.inbox) {
            (Some(pane), _) => self.agent_in(pane),
            (None, Some(inbox)) => self.answers(inbox),
            (None, None) => true,
        }
    }

    fn activity(&self, participant: &Participant) -> Option<Activity> {
        participant.pane.as_ref().and_then(|pane| self.agents.borrow().get(pane).copied())
    }

    fn agent_in(&self, pane: &str) -> bool {
        self.agents.borrow().contains_key(pane)
    }

    fn has_pane(&self, pane: &str) -> bool {
        self.agent_in(pane) || self.open.borrow().contains_key(pane)
    }

    fn attended(&self) -> bool {
        self.attended.get()
    }

    fn doorbell(&self, pane: &str) -> Ringable {
        if self.agent_in(pane) && self.unreadable.borrow().contains(pane) {
            Ringable::NoPrompt
        } else if self.agent_in(pane) {
            Ringable::Rings
        } else if self.open.borrow().get(pane) == Some(&true) {
            Ringable::AgentToCome
        } else {
            Ringable::NoAgent
        }
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
        pane: text("pane"),
        directory: text("directory"),
    }
}

fn notice(notice: &Notice) -> String {
    format!(
        "{} #{}-{} x{} to-you:{} from {}{}",
        notice.group,
        notice.first,
        notice.last,
        notice.count,
        notice.to_you,
        notice.from.join(","),
        if notice.again { " again" } else { "" }
    )
}

fn activity(activity: Activity) -> &'static str {
    match activity {
        Activity::Working => "working",
        Activity::Blocked => "blocked",
        Activity::Idle => "idle",
        Activity::Waiting => "waiting",
    }
}

/// Where a wake went, when it went to a pane or the windows rather than an inbox.
fn rung(via: &Via) -> String {
    match via {
        Via::Inbox(_) => String::new(),
        Via::Pane(pane) => format!(" rung {pane}"),
        Via::Human => " for the windows".to_string(),
    }
}

fn refused(refusal: &Refusal) -> String {
    let detail = match refusal {
        Refusal::BadName { name } | Refusal::NoSuchParticipant { name } => name.clone(),
        Refusal::NotAParticipant { name } => {
            name.clone().unwrap_or_else(|| "this session".to_string())
        }
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
/// the others gone, and leaving a pane whose agent is still to come to ring once it is - and
/// says what the post did for each participant it was meant to wake.
fn delivered(service: &mut Messaging<Memory>, sessions: &Sessions, posted: &Posted) -> String {
    let mut failed = BTreeSet::new();
    let mut deferred = BTreeSet::new();
    for wake in &posted.wakes {
        let reached = match &wake.via {
            Via::Inbox(inbox) => sessions.answers(inbox),
            Via::Pane(pane) if sessions.doorbell(pane) == Ringable::AgentToCome => {
                deferred.insert(wake.name.clone());
                continue;
            }
            Via::Pane(pane) => sessions.agent_in(pane),
            // Told to whichever windows attend, now or later: nothing to fail.
            Via::Human => continue,
        };
        service.delivered(wake, reached).unwrap();
        if !reached {
            failed.insert(wake.name.clone());
        }
    }
    let mut parts = vec![format!("{} posted #{} to {}", posted.author, posted.seq, posted.group)];
    for (label, wanted) in [
        ("woke", Reach::Woken),
        ("deferred", Reach::Deferred),
        ("already woken", Reach::AlreadyWoken),
        ("waiting", Reach::Waiting),
        ("gone", Reach::Gone),
        ("no agent", Reach::NoAgent),
        ("no doorbell", Reach::NoDoorbell),
    ] {
        let names: Vec<String> = posted
            .reached
            .iter()
            .filter(|(name, reach)| {
                let reach = if failed.contains(name) {
                    Reach::Gone
                } else if deferred.contains(name) {
                    Reach::Deferred
                } else {
                    *reach
                };
                reach == wanted
            })
            .map(|(name, _)| {
                let told = posted
                    .wakes
                    .iter()
                    .find(|wake| wake.name == *name && !failed.contains(name))
                    .map(|wake| (&wake.notice, rung(&wake.via)));
                let answered = posted
                    .answered
                    .iter()
                    .find(|answered| answered.name == *name)
                    .map(|answered| (&answered.notice, String::new()));
                match told.or(answered) {
                    Some((told, rung)) => format!("{name} [{}]{rung}", notice(told)),
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

/// Opens a pane with no agent in it, `new` when one is likely starting there, or closes it.
fn pane(sessions: &Sessions, step: &Value) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or_default();
    let pane = text("pane").to_string();
    match text("state") {
        "new" => sessions.open.borrow_mut().insert(pane.clone(), true),
        "old" => sessions.open.borrow_mut().insert(pane.clone(), false),
        _ => sessions.open.borrow_mut().remove(&pane),
    };
    format!("{pane} {}", text("state"))
}

/// Says what a pane's agent is doing, or with `gone` that the pane has none. `unreadable` makes
/// it an idle agent whose prompt the host cannot read.
fn agent(sessions: &Sessions, step: &Value) -> String {
    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or_default();
    let pane = text("pane").to_string();
    let state = match text("state") {
        "working" => Some(Activity::Working),
        "blocked" => Some(Activity::Blocked),
        "idle" => Some(Activity::Idle),
        "waiting" => Some(Activity::Waiting),
        "unreadable" => {
            sessions.unreadable.borrow_mut().insert(pane.clone());
            Some(Activity::Idle)
        }
        _ => None,
    };
    match state {
        Some(state) => sessions.agents.borrow_mut().insert(pane.clone(), state),
        None => sessions.agents.borrow_mut().remove(&pane),
    };
    format!("{pane} {}", text("state"))
}

/// Tells the service `name`'s agent went idle, and says whom that woke again.
fn idle(service: &mut Messaging<Memory>, sessions: &Sessions, name: &str) -> String {
    let (wakes, _) = service.went_idle(name, sessions);
    let told: Vec<String> = wakes
        .iter()
        .map(|wake| format!("{} [{}]{}", wake.name, notice(&wake.notice), rung(&wake.via)))
        .collect();
    if told.is_empty() {
        "rewoke nobody".to_string()
    } else {
        format!("rewoke {}", told.join(", "))
    }
}

/// A window starts attending, or with `off` stops.
fn attend(sessions: &Sessions, attending: bool) -> String {
    sessions.attended.set(attending);
    if attending { "attended" } else { "unattended" }.to_string()
}

/// What the windows would be told waits for the human.
fn waits_for_the_human(service: &Messaging<Memory>) -> String {
    let notices = service.human_notices();
    if notices.is_empty() {
        return "nothing waits for @human".to_string();
    }
    format!("@human: {}", notices.iter().map(notice).collect::<Vec<_>>().join("; "))
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
        "leave" => service.leave(&who, text("group"), sessions, now).map(|left| {
            let line = if left.stopped && left.groups.is_empty() {
                format!("{} stopped", left.name)
            } else if left.stopped {
                format!("{} stopped (left {})", left.name, left.groups.join(","))
            } else {
                format!("{} left {}", left.name, left.groups.join(","))
            };
            match left.ended {
                Some(ticket) => format!("{line}, ended wait {ticket}"),
                None => line,
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
                    match member.activity {
                        Some(doing) => format!("{} {liveness} ({})", member.name, activity(doing)),
                        None => format!("{} {liveness}", member.name),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "log" => {
            let since = step.get("since").and_then(Value::as_u64).unwrap_or(0);
            service.log(text("group").unwrap_or_default(), since).map(|entries| list(&entries))
        }
        "agent" => Ok(agent(sessions, step)),
        "pane" => Ok(pane(sessions, step)),
        "idle" => Ok(idle(service, sessions, text("name").unwrap_or_default())),
        "attend" => Ok(attend(sessions, text("state") != Some("off"))),
        "human" => Ok(waits_for_the_human(service)),
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
