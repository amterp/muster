//! The banners for messages to the human: which stand, what each shows, and what the human's
//! answers and dismissals do to them. Cases live in corpus/conformance/banners.json.
//!
//! Every case is a fold over steps, because a banner is: whether a message replaces the one
//! shown depends on what the human was doing with it when the message arrived.

use conformance::{CaseError, Conformance, fields, strings};
use muster_core::attention::{GroupKey, HumanNotice};
use muster_core::banners::{Banner, Banners, Message, Shown};
use muster_core::composition::DaemonId;
use muster_core::posting::Sent;
use serde_json::{Map, Value, json};

#[test]
fn banners_conformance() {
    let corpus = Conformance::load("banners.json");
    let ran = corpus.run(|given| {
        let steps = given
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| CaseError::new("`steps` is missing"))?;
        let mut banners = Banners::new();
        let (mut fetched, mut said) = (Vec::new(), Vec::new());
        for step in steps {
            take(&mut banners, step, &mut fetched, &mut said)?;
        }
        let stack = banners.stack();
        Ok(fields([
            ("fetch", (!fetched.is_empty()).then(|| json!(fetched))),
            ("said", (!said.is_empty()).then(|| json!(said))),
            ("stack", Some(Value::Array(stack.shown.iter().map(banner_json).collect()))),
            ("folded", (stack.folded > 0).then(|| json!(stack.folded))),
            ("oldest", stack.oldest.as_ref().map(|group| json!(spelled(group)))),
        ]))
    });
    assert_eq!(ran, corpus.cases.len());
}

fn take(
    banners: &mut Banners,
    step: &Value,
    fetched: &mut Vec<String>,
    said: &mut Vec<String>,
) -> Result<(), CaseError> {
    let seq = || number(step, "seq");
    let mut answer = |result: Result<String, String>| match result {
        Ok(text) if text.is_empty() => {}
        Ok(text) => said.push(text),
        Err(why) => said.push(format!("refused: {why}")),
    };
    if let Some(group) = text(step, "notice") {
        let group = key(group);
        let notice = HumanNotice {
            last: number(step, "last").unwrap_or(0),
            count: number(step, "count").unwrap_or(0),
            from: strings(step, "from"),
            member: true,
            ..HumanNotice::default()
        };
        let looking = step.get("looking").and_then(Value::as_bool).unwrap_or(false);
        if let Some(seq) = banners.noticed(&group, Some(&notice), looking) {
            fetched.push(format!("{}#{seq}", spelled(&group)));
        }
    } else if let Some(group) = text(step, "fetched") {
        let seq = seq().ok_or_else(|| CaseError::new("`fetched` needs `seq`"))?;
        let result = match text(step, "error") {
            Some(why) => Err(why.to_string()),
            None => Ok(Message::new(
                seq,
                text(step, "author").unwrap_or_default().to_string(),
                vec!["@human".to_string()],
                step.get("urgent").and_then(Value::as_bool).unwrap_or(false),
                0,
                text(step, "body").unwrap_or_default(),
            )),
        };
        banners.fetched(&key(group), seq, result);
    } else if let Some(group) = text(step, "hold") {
        let held = step.get("held").and_then(Value::as_bool).unwrap_or(true);
        let seq = seq().ok_or_else(|| CaseError::new("`hold` needs `seq`"))?;
        answer(banners.hold(&key(group), seq, held).map(|_| String::new()));
    } else if let Some(group) = text(step, "reply") {
        let seq = seq().ok_or_else(|| CaseError::new("`reply` needs `seq`"))?;
        let body = text(step, "body").unwrap_or_default();
        answer(banners.reply(&key(group), seq, body).map(|to| format!("to {}", to.join(", "))));
    } else if let Some(group) = text(step, "replied") {
        let seq = seq().ok_or_else(|| CaseError::new("`replied` needs `seq`"))?;
        let body = text(step, "body").unwrap_or_default().to_string();
        let sent = if let Some(posted) = number(step, "posted") {
            let heard = step.get("heard").and_then(Value::as_bool).unwrap_or(true);
            let said = text(step, "said").unwrap_or_default().to_string();
            Sent::Posted { seq: posted, said, heard }
        } else if let Some(why) = text(step, "refused") {
            Sent::Refused { body, why: why.to_string() }
        } else if let Some(why) = text(step, "unknown") {
            Sent::Unknown { body, why: why.to_string() }
        } else {
            return Err(CaseError::new("`replied` needs `posted`, `refused` or `unknown`"));
        };
        banners.replied(&key(group), seq, sent);
    } else if let Some(group) = text(step, "dismiss") {
        let seq = seq().ok_or_else(|| CaseError::new("`dismiss` needs `seq`"))?;
        answer(banners.dismiss(&key(group), seq).map(|()| String::new()));
    } else if let Some(allowed) = step.get("notifying").and_then(Value::as_bool) {
        banners.notifying(allowed);
    } else if let Some(daemon) = text(step, "detach") {
        banners.detached(&DaemonId::new(daemon));
    } else {
        return Err(CaseError::new(format!("a step this driver does not know: {step}")));
    }
    Ok(())
}

fn banner_json(banner: &Banner) -> Value {
    let mut fields = Map::new();
    fields.insert("group".into(), json!(spelled(&banner.group)));
    fields.insert("seq".into(), json!(banner.shown.seq()));
    match &banner.shown {
        Shown::Message(message) => {
            fields.insert("author".into(), json!(message.author));
            if message.urgent {
                fields.insert("urgent".into(), json!(true));
            }
            if message.more {
                fields.insert("more".into(), json!(true));
            }
        }
        Shown::Unloaded { from, count, why, .. } => {
            fields.insert(
                "unloaded".into(),
                json!(format!("{count} from {}: {why}", from.join(", "))),
            );
        }
        Shown::Loading { .. } => {
            fields.insert("loading".into(), json!(true));
        }
    }
    if banner.held {
        fields.insert("held".into(), json!(true));
    }
    if let Some(sent) = &banner.sent {
        let sent = match sent {
            Sent::Pending { body } => format!("pending: {body}"),
            Sent::Posted { seq, said, heard: true } => format!("posted #{seq}: {said}"),
            Sent::Posted { seq, said, heard: false } => format!("posted #{seq}, unheard: {said}"),
            Sent::Refused { body, why } => format!("refused: {why} (keeps {body:?})"),
            Sent::Unknown { body, why } => format!("unknown: {why} (keeps {body:?})"),
        };
        fields.insert("sent".into(), json!(sent));
    }
    if let Some(waiting) = &banner.waiting {
        let urgent = if waiting.urgent { ", urgent" } else { "" };
        fields.insert(
            "waiting".into(),
            json!(format!("#{} from {}{urgent}", waiting.seq, waiting.author)),
        );
    }
    fields.insert("replyable".into(), json!(banner.replyable));
    Value::Object(fields)
}

/// `g` is a group on the daemon `local`, and `devenv/g` one on `devenv`.
fn key(spelled: &str) -> GroupKey {
    match spelled.split_once('/') {
        Some((daemon, group)) => GroupKey { daemon: DaemonId::new(daemon), group: group.into() },
        None => GroupKey { daemon: DaemonId::new("local"), group: spelled.into() },
    }
}

fn spelled(group: &GroupKey) -> String {
    if group.daemon.as_str() == "local" { group.group.clone() } else { group.to_string() }
}

fn text<'a>(step: &'a Value, name: &str) -> Option<&'a str> {
    step.get(name).and_then(Value::as_str)
}

fn number(step: &Value, name: &str) -> Option<u64> {
    step.get(name).and_then(Value::as_u64)
}
