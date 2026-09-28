//! Which agents have been seen, and which of them are worth interrupting somebody for.
//!
//! Two corpora, one fold. `attention.json` is seen-ness - which panes present as `done` -
//! and `attention-notifying.json` is the unread set laid over it. They are separate files
//! because they are separate claims: seen-ness is what a pane *is*, and is not a person's to
//! configure, while notifying is what Muster does about it and is. A case about one would
//! otherwise have to state the other, and every existing seen-ness case would carry an
//! assertion it was not written to make.
//!
//! Every case is a fold over a sequence, because both are: what a pane's record means depends
//! on what the window was showing when it arrived.

use std::collections::{BTreeMap, BTreeSet};

use conformance::{CaseError, Conformance, fields};
use muster_core::AgentState;
use muster_core::attention::{
    Attend, Attention, GroupKey, HumanNotice, Note, Noticed, Notifications,
};
use muster_core::composition::{DaemonId, PaneKey};
use muster_core::mirror::backend::PaneId;
use serde_json::{Map, Value, json};

#[test]
fn attention_conformance() {
    let corpus = Conformance::load("attention.json");

    let ran = corpus.run(|given| {
        let run = fold(given)?;
        let states: Map<String, Value> = run
            .backend
            .iter()
            .map(|(pane, state)| {
                (pane.to_string(), json!(run.attention.presented(pane, *state).as_str()))
            })
            .collect();
        Ok(fields([
            ("states", Some(Value::Object(states))),
            ("reported", Some(json!(run.reported))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn notifying_conformance() {
    let corpus = Conformance::load("attention-notifying.json");

    let ran = corpus.run(|given| {
        let run = fold(given)?;
        Ok(fields([
            // The sequence, because a notification is an event: what has to be right is which
            // moments produce one, and an end state cannot say whether somebody was
            // interrupted twice on the way there.
            ("notified", Some(json!(run.notified))),
            // Panes marked by a bell nobody has seen, when any are: a mark and never a banner.
            ("rung", {
                let rung: Vec<String> = run
                    .backend
                    .keys()
                    .filter(|pane| run.attention.has_rung(pane))
                    .map(ToString::to_string)
                    .collect();
                (!rung.is_empty()).then(|| json!(rung))
            }),
            // And every pane still asking, in the order the chord to the pane that asked walks
            // them - with a banner or without one.
            (
                "asking",
                Some(json!(
                    run.attention
                        .asking()
                        .iter()
                        .map(|(asker, alert)| format!("{asker} {}", alert.as_str()))
                        .collect::<Vec<_>>()
                )),
            ),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// One case's sequence, applied.
struct Run {
    attention: Attention,
    /// The daemon's last word per pane, tracked here rather than held by `Attention`: the
    /// mirror is where a backend's state lives, and a second copy inside the thing that
    /// presents it would be a second copy to disagree with.
    backend: BTreeMap<PaneKey, AgentState>,
    /// Every notification the run raised or withdrew, in order.
    notified: Vec<String>,
    /// Every pane the window reported seen to its daemon, in order.
    reported: Vec<String>,
}

fn fold(given: &Value) -> Result<Run, CaseError> {
    let mut run = Run {
        attention: Attention::new(),
        backend: BTreeMap::new(),
        notified: Vec::new(),
        reported: Vec::new(),
    };
    if let Some(notifications) = read_notifications(given) {
        run.attention.notifying(notifications);
    }
    if let Some(focused) = given.get("focused").and_then(Value::as_bool) {
        run.attention.window_focused(focused);
    }
    let noticed = run.attention.showing(read_panes(given, "visible")?);
    record(&mut run, &noticed);

    for event in given.get("events").and_then(Value::as_array).into_iter().flatten() {
        // A file saved while the window is up. Its own step because the rule it proves is
        // asymmetric: what a new answer silences goes now, and what it un-silences does not
        // come back.
        if let Some(notifications) = read_notifications(event) {
            let stale = run.attention.notifying(notifications);
            run.notified.extend(stale.iter().map(|asker| format!("{asker} withdrawn")));
            continue;
        }
        if let Some(focused) = event.get("focused").and_then(Value::as_bool) {
            let noticed = run.attention.window_focused(focused);
            record(&mut run, &noticed);
            continue;
        }
        if event.get("visible").is_some() {
            let noticed = run.attention.showing(read_panes(event, "visible")?);
            record(&mut run, &noticed);
            continue;
        }
        // A daemon's connection came back, so what the window reported to it may be lost.
        if let Some(daemon) = event.get("reconnected").and_then(Value::as_str) {
            let noticed = run.attention.reconnected(&DaemonId::new(daemon));
            record(&mut run, &noticed);
            continue;
        }
        // A daemon refused the window's report that it saw these panes, as one handing its
        // panes to another does.
        if event.get("refused").is_some() {
            let pane = read_pane(event, "refused")?;
            run.attention.refused(std::slice::from_ref(&pane));
            continue;
        }
        // A pane the backend no longer holds. Its own step because what it proves is
        // about the id rather than about a state: ids are reused, so what is remembered
        // about a closed pane is inherited by the next one to be given its name.
        if event.get("closed").is_some() {
            let pane = read_pane(event, "closed")?;
            if run.attention.forget(&pane).is_some() {
                run.notified.push(withdrawn(&pane));
            }
            run.backend.remove(&pane);
            continue;
        }
        // A pane this window is meeting for the first time, as the daemon already had
        // it - which is what every pane looks like on the way up.
        if event.get("appeared").is_some() {
            let pane = read_pane(event, "appeared")?;
            let state = read_state(event, "state")?;
            if run.attention.met(&pane, state, finished(event)) {
                run.reported.push(pane.to_string());
            }
            run.backend.insert(pane, state);
            continue;
        }
        // What a group's daemon says now waits for the human there (MIP-4, section 10), with
        // `looking` when the group's transcript is on screen in a focused window.
        if let Some(message) = event.get("message") {
            messaged(&mut run, event, message)?;
            continue;
        }
        // A program in the pane rang the bell.
        if event.get("rang").is_some() {
            let pane = read_pane(event, "rang")?;
            run.attention.bell(&pane);
            continue;
        }
        // A program in the pane asked for a notification, and said what: the pane's agent, when
        // the case names the one its daemon recognized there.
        if let Some(note) = event.get("notification") {
            let pane = read_pane(event, "pane")?;
            let said = |key: &str| note.get(key).and_then(Value::as_str).unwrap_or_default();
            let note = Note { title: said("title").to_string(), body: said("body").to_string() };
            let agent = event.get("agent").and_then(Value::as_str);
            if let Some(Attend::Raised(alert)) = run.attention.notified(&pane, note, agent) {
                let body = run.attention.note(&pane).map(|note| note.body.clone());
                run.notified.push(format!(
                    "{pane} {}: {}",
                    alert.as_str(),
                    body.unwrap_or_default()
                ));
            }
            continue;
        }
        // A pane's record as its daemon now has it.
        let pane = read_pane(event, "pane")?;
        let state = read_state(event, "state")?;
        let observed = run.attention.observed(&pane, state, finished(event));
        match observed.attend {
            Some(Attend::Raised(alert)) => {
                run.notified.push(format!("{pane} {}", alert.as_str()));
            }
            Some(Attend::Withdrawn) => run.notified.push(withdrawn(&pane)),
            None => {}
        }
        if observed.reported {
            run.reported.push(pane.to_string());
        }
        run.backend.insert(pane, state);
    }
    Ok(run)
}

/// Tells the window what a group's daemon says now waits for the human there.
fn messaged(run: &mut Run, event: &Value, message: &Value) -> Result<(), CaseError> {
    let group = read_group(event)?;
    let number = |key: &str| message.get(key).and_then(Value::as_u64).unwrap_or_default();
    let notice = HumanNotice {
        last: number("last"),
        count: number("count"),
        to_you: number("to_you"),
        from: message
            .get("from")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|from| from.as_str().map(str::to_string))
            .collect(),
    };
    let looking = event.get("looking").and_then(Value::as_bool).unwrap_or_default();
    match run.attention.messaged(&group, Some(notice), looking) {
        Some(Attend::Raised(alert)) => {
            let from = run.attention.human_notice(&group).map(|notice| notice.from.join(", "));
            run.notified.push(format!("{group} {}: {}", alert.as_str(), from.unwrap_or_default()));
        }
        Some(Attend::Withdrawn) => run.notified.push(format!("{group} withdrawn")),
        None => {}
    }
    Ok(())
}

fn record(run: &mut Run, noticed: &Noticed) {
    run.notified.extend(noticed.withdrawn.iter().map(withdrawn));
    run.reported.extend(noticed.reported.iter().map(ToString::to_string));
}

/// Whether the record carries the daemon's `finished_unseen`, which a case says only when it does.
fn finished(event: &Value) -> bool {
    event.get("finished").and_then(Value::as_bool).unwrap_or_default()
}

fn withdrawn(pane: &PaneKey) -> String {
    format!("{pane} withdrawn")
}

/// `{ "notifications": { "blocked": true, "done": false, "muted": false } }`, or nothing.
///
/// Partial, the way the file is: a case naming one key changes one answer and leaves the
/// other two at what Muster ships.
fn read_notifications(given: &Value) -> Option<Notifications> {
    let block = given.get("notifications")?.as_object()?;
    let mut notifications = Notifications::default();
    for (key, held) in [
        ("blocked", &mut notifications.blocked),
        ("programs", &mut notifications.programs),
        ("messages", &mut notifications.messages),
        ("done", &mut notifications.done),
        ("muted", &mut notifications.muted),
    ] {
        if let Some(said) = block.get(key).and_then(Value::as_bool) {
            *held = said;
        }
    }
    Some(notifications)
}

/// `local/w1:p1` - a daemon and a pane in one token.
///
/// Split at the first slash rather than the last, because a pane id is the backend's own
/// string and Muster never parses it. A daemon id is Muster's own and holds no slash.
fn read_key(text: &str) -> Result<PaneKey, CaseError> {
    let (daemon, pane) = text.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{text}` names no daemon - a pane is written `local/w1:p1`"))
    })?;
    Ok(PaneKey { daemon: DaemonId::new(daemon), pane: PaneId::new(pane) })
}

/// `local/review` - a daemon and one of its groups, in one token, from the event's `group`.
fn read_group(event: &Value) -> Result<GroupKey, CaseError> {
    let text = event
        .get("group")
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new("a message event has no `group`"))?;
    let (daemon, group) = text.split_once('/').ok_or_else(|| {
        CaseError::new(format!("`{text}` names no daemon - a group is written `local/review`"))
    })?;
    Ok(GroupKey { daemon: DaemonId::new(daemon), group: group.to_string() })
}

fn read_pane(given: &Value, key: &str) -> Result<PaneKey, CaseError> {
    let text = given
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))?;
    read_key(text)
}

fn read_panes(given: &Value, key: &str) -> Result<BTreeSet<PaneKey>, CaseError> {
    given
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| {
            entry
                .as_str()
                .ok_or_else(|| {
                    CaseError::new(format!("`{key}` holds something that is not a pane"))
                })
                .and_then(read_key)
        })
        .collect()
}

fn read_state(given: &Value, key: &str) -> Result<AgentState, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(AgentState::from_backend)
        .ok_or_else(|| CaseError::new(format!("the case has no `{key}`")))
}
