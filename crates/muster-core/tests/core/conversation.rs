//! A group's conversation: the thread's rows, and the message box's draft. Cases live in
//! corpus/conformance/conversation.json and corpus/conformance/message_box.json.
//!
//! Both build a group the same way - its log, members, callsigns and policy - so they share a
//! driver.

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields, strings};
use muster_core::composition::PaneKey;
use muster_core::conversation::{
    Addressed, Change, Draft, Edit, Entry, Group, Liveness, Member, Message, Policy, Reading, Row,
    What, draft, thread,
};
use serde_json::{Value, json};

#[test]
fn conversation_conformance() {
    let corpus = Conformance::load("conversation.json");
    let ran = corpus.run(|given| {
        let group = group(given)?;
        let reading = Reading {
            unread_from: given.get("unread_from").and_then(Value::as_u64),
            open_folds: numbers(given, "open"),
            whole: numbers(given, "whole"),
            utc_offset_s: given
                .get("offset")
                .and_then(Value::as_i64)
                .and_then(|offset| i32::try_from(offset).ok())
                .unwrap_or(0),
        };
        let rows = thread::rows(&group, &reading);
        Ok(json!({ "rows": rows.iter().map(row_json).collect::<Vec<_>>() }))
    });
    assert_eq!(ran, corpus.cases.len());
}

#[test]
fn message_box_conformance() {
    let corpus = Conformance::load("message_box.json");
    let ran = corpus.run(|given| {
        let group = group(given)?;
        let Some(edit) = given.get("edit") else {
            let opened = draft::opening(&group);
            return Ok(json!({ "to": opened.to }));
        };
        let before = given.get("draft").map(read_draft).unwrap_or_default();
        let text = text(edit, "text").unwrap_or_default().to_string();
        let edit = Edit {
            cursor: edit
                .get("cursor")
                .and_then(Value::as_u64)
                .and_then(|cursor| usize::try_from(cursor).ok())
                .unwrap_or(text.chars().count()),
            text,
            to: edit.get("to").map_or_else(|| before.to.clone(), |_| strings(edit, "to")),
            urgent: edit.get("urgent").and_then(Value::as_bool).unwrap_or(before.urgent),
            unaddress: edit.get("unaddress").and_then(Value::as_bool).unwrap_or(false),
        };
        let edited = draft::edited(&before, edit, &group);
        Ok(fields([
            ("text", Some(json!(edited.draft.text))),
            ("to", Some(json!(edited.draft.to))),
            ("cursor", Some(json!(edited.cursor))),
            ("addressed", Some(json!(addressed_name(edited.draft.addressed)))),
            ("urgent", edited.draft.urgent.then_some(json!(true))),
            (
                "completions",
                (!edited.completions.is_empty()).then(|| {
                    json!(edited.completions.iter().map(|c| c.shown.clone()).collect::<Vec<_>>())
                }),
            ),
        ]))
    });
    assert_eq!(ran, corpus.cases.len());
}

fn group(given: &Value) -> Result<Group, CaseError> {
    let mut group = Group::default();
    let entries = given
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(entry)
        .collect::<Result<Vec<_>, _>>()?;
    group.log.page(entries, false);
    if let Some(members) = given.get("members").and_then(Value::as_array) {
        group.members = Some(
            members
                .iter()
                .filter_map(Value::as_str)
                .map(|name| Member {
                    name: name.to_string(),
                    liveness: if name == "@human" { Liveness::Human } else { Liveness::Alive },
                    pane: given
                        .get("panes")
                        .and_then(|panes| panes.get(name))
                        .and_then(Value::as_str)
                        .map(|pane| PaneKey::new(&"local".into(), &pane.into())),
                    unread: None,
                })
                .collect(),
        );
    }
    if let Some(called) = given.get("called").and_then(Value::as_object) {
        group.called = called
            .iter()
            .filter_map(|(name, callsign)| Some((name.clone(), callsign.as_str()?.to_string())))
            .collect();
    }
    if let Some(policy) = given.get("policy") {
        let ring: BTreeMap<String, Vec<String>> = policy
            .get("ring")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(author, _)| (author.clone(), strings(&policy["ring"], author)))
            .collect();
        let paused = policy.get("paused").and_then(Value::as_bool).unwrap_or(false);
        group.policy = Some(Policy { ring, paused });
    }
    Ok(group)
}

/// An entry as a case spells it: `{ "seq": 4, "author": "a", "to": ["@human"], "body": "..." }`
/// for a message, or one of `joined`, `left`, `created`, or `changed` with `by`.
fn entry(given: &Value) -> Result<Entry, CaseError> {
    let seq = given
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| CaseError::new("an entry needs `seq`"))?;
    let at_ms = given.get("at_ms").and_then(Value::as_u64).unwrap_or(seq * 1000);
    let what = if let Some(name) = text(given, "joined") {
        What::Joined(name.to_string())
    } else if let Some(name) = text(given, "left") {
        What::Left(name.to_string())
    } else if let Some(name) = text(given, "created") {
        What::Created(name.to_string())
    } else if let Some(change) = text(given, "changed") {
        let change = match change {
            "policy" => Change::Policy,
            "paused" => Change::Paused,
            "resumed" => Change::Resumed,
            other => return Err(CaseError::new(format!("no change called `{other}`"))),
        };
        What::Changed { by: text(given, "by").unwrap_or_default().to_string(), change }
    } else {
        let body = match given.get("lines").and_then(Value::as_u64) {
            Some(lines) => {
                (1..=lines).map(|n| format!("line {n}\n")).collect::<Vec<_>>().join("\n")
            }
            None => text(given, "body").unwrap_or("hello").to_string(),
        };
        What::Message(Message::new(
            text(given, "author").ok_or_else(|| CaseError::new("a message needs `author`"))?.into(),
            strings(given, "to"),
            body,
            given.get("urgent").and_then(Value::as_bool).unwrap_or(false),
        ))
    };
    Ok(Entry { seq, at_ms, what })
}

fn row_json(row: &Row) -> Value {
    match row {
        Row::Divider { date } => json!({ "divider": date }),
        Row::Members { text, .. } => json!({ "members": text }),
        Row::Folded { first, count, .. } => json!({ "folded": first, "count": count }),
        Row::Unread => json!("unread"),
        Row::Message(message) => fields([
            ("message", Some(json!(message.seq))),
            ("author", Some(json!(message.author))),
            ("to", (!message.to.is_empty()).then(|| json!(message.to))),
            ("to_you", message.to_you.then_some(json!(true))),
            ("by_you", message.by_you.then_some(json!(true))),
            ("urgent", message.urgent.then_some(json!(true))),
            ("more", message.more.then_some(json!(true))),
            ("no_reply", (!message.replyable).then_some(json!(true))),
            ("muster", message.from_muster.then_some(json!(true))),
            ("pane", message.author_pane.as_ref().map(|pane| json!(pane.pane.as_str()))),
        ]),
    }
}

fn read_draft(given: &Value) -> Draft {
    Draft {
        text: text(given, "text").unwrap_or_default().to_string(),
        to: strings(given, "to"),
        urgent: given.get("urgent").and_then(Value::as_bool).unwrap_or(false),
        addressed: match text(given, "addressed") {
            Some("chosen") => Addressed::Chosen,
            Some("at") => Addressed::At,
            _ => Addressed::Opening,
        },
    }
}

fn addressed_name(addressed: Addressed) -> &'static str {
    match addressed {
        Addressed::Opening => "opening",
        Addressed::Chosen => "chosen",
        Addressed::At => "at",
    }
}

fn numbers(given: &Value, key: &str) -> BTreeSet<u64> {
    given
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect()
}

fn text<'a>(given: &'a Value, key: &str) -> Option<&'a str> {
    given.get(key).and_then(Value::as_str)
}
