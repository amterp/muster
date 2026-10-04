//! The mirror is the core's picture of daemon truth, and everything above it renders from
//! that picture. Cases and their reasoning live in corpus/conformance/mirror.json.

use crate::support::backend::{read_human_notice, read_pane, read_snapshot, read_tab, text};
use conformance::{Conformance, fields};
use muster_core::mirror::backend::{PaneId, Progress, ProgressState, TabId};
use muster_core::mirror::{BackendEvent, Change, Mirror, Restored};
use serde_json::{Map, Value, json};

#[test]
fn mirror_conformance() {
    let corpus = Conformance::load("mirror.json");

    let ran = corpus.run(|given| {
        let mut mirror = Mirror::new();
        let mut changes = Vec::new();

        // Snapshot, then events, then snapshot again - the order a real connection takes, so
        // that a case about a reconnect is a case about what the mirror had been told before
        // the drop rather than about a bare pair of snapshots.
        if let Some(snapshot) = given.get("snapshot") {
            mirror.bootstrap(read_snapshot(snapshot));
        }
        for event in given.get("events").and_then(Value::as_array).into_iter().flatten() {
            changes.extend(mirror.apply(read_event(event)));
        }
        if let Some(resnapshot) = given.get("resnapshot") {
            changes.extend(mirror.bootstrap(read_snapshot(resnapshot)));
        }
        // What arrives after a reconnect: a new run still restoring says it has finished.
        for event in given.get("afterEvents").and_then(Value::as_array).into_iter().flatten() {
            changes.extend(mirror.apply(read_event(event)));
        }

        // Every field, every case. The corpus compares whole objects, and for a state machine
        // that is the point: a case asserting only what it is about would miss a change that
        // also clobbered something else.
        Ok(fields([
            ("panes", Some(json!(mirror.panes().map(|p| p.id.to_string()).collect::<Vec<_>>()))),
            ("tabs", Some(json!(mirror.tabs().map(|t| t.id.to_string()).collect::<Vec<_>>()))),
            ("paneTabs", Some(pane_tabs(&mirror))),
            ("agentStates", Some(agent_states(&mirror))),
            ("names", named(&mirror)),
            ("tabLabels", tab_labels(&mirror)),
            ("layouts", Some(layouts(&mirror))),
            ("health", Some(json!(mirror.health().as_str()))),
            ("restoring", Some(json!(mirror.restoring()))),
            ("progress", progress(&mirror)),
            ("human", human(&mirror)),
            ("changes", Some(json!(changes.iter().map(describe).collect::<Vec<_>>()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn pane_tabs(mirror: &Mirror) -> Value {
    let mut map = Map::new();
    for pane in mirror.panes() {
        map.insert(pane.id.to_string(), json!(pane.tab.as_str()));
    }
    Value::Object(map)
}

/// The name somebody gave a pane and the title its program set, for the panes that have
/// either. Absent when no pane in the case has one, so a case about structure does not carry a
/// map of empty objects.
fn named(mirror: &Mirror) -> Option<Value> {
    let mut map = Map::new();
    for pane in mirror.panes() {
        let described = fields([
            ("name", pane.name.as_ref().map(|name| json!(name))),
            ("title", pane.title.as_ref().map(|title| json!(title))),
        ]);
        if described.as_object().is_some_and(|held| !held.is_empty()) {
            map.insert(pane.id.to_string(), described);
        }
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

/// What each named tab is called, absent when none is.
fn tab_labels(mirror: &Mirror) -> Option<Value> {
    let mut map = Map::new();
    for tab in mirror.tabs() {
        if let Some(label) = &tab.label {
            map.insert(tab.id.to_string(), json!(label));
        }
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

fn agent_states(mirror: &Mirror) -> Value {
    let mut map = Map::new();
    for pane in mirror.panes() {
        map.insert(pane.id.to_string(), json!(pane.agent_state.as_str()));
    }
    Value::Object(map)
}

/// What each pane's program says of its progress, when any says anything.
fn progress(mirror: &Mirror) -> Option<Value> {
    let said: Map<String, Value> = mirror
        .panes()
        .filter_map(|pane| {
            let progress = mirror.progress(&pane.id)?;
            let percent =
                progress.percent.map(|percent| format!(" {percent}%")).unwrap_or_default();
            Some((pane.id.to_string(), json!(format!("{}{percent}", progress.state.as_str()))))
        })
        .collect();
    (!said.is_empty()).then_some(Value::Object(said))
}

/// What waits for the human, per group, when anything does.
fn human(mirror: &Mirror) -> Option<Value> {
    let said: Map<String, Value> = mirror
        .human_notices()
        .map(|(group, notice)| {
            let line =
                format!("x{} last #{} from {}", notice.count, notice.last, notice.from.join(","));
            (group.clone(), json!(line))
        })
        .collect();
    (!said.is_empty()).then_some(Value::Object(said))
}

/// Each tab's tree on one line, keyed by tab.
fn layouts(mirror: &Mirror) -> Value {
    let mut map = Map::new();
    for tab in mirror.tabs() {
        let mut described = tab.root.to_string();
        if let Some(zoomed) = &tab.zoomed {
            described = format!("{described} zoomed={zoomed}");
        }
        map.insert(tab.id.to_string(), json!(described));
    }
    Value::Object(map)
}

/// Changes render as readable strings rather than as nested objects, because a corpus is read
/// by people deciding whether an expectation is right (docs/testing.md: bytes render readably).
fn describe(change: &Change) -> String {
    match change {
        Change::PaneAdded(pane) => format!("paneAdded:{pane}"),
        Change::PaneRemoved(pane) => format!("paneRemoved:{pane}"),
        Change::AgentStateChanged { pane, from, to } => {
            format!("agentStateChanged:{pane}:{}->{}", from.as_str(), to.as_str())
        }
        Change::FinishedUnseen { pane, unseen } => format!("finishedUnseen:{pane}:{unseen}"),
        Change::PaneRelabelled(pane) => format!("paneRelabelled:{pane}"),
        Change::AgentDescribed(pane) => format!("agentDescribed:{pane}"),
        Change::TabAdded(tab) => format!("tabAdded:{tab}"),
        Change::TabRelabelled(tab) => format!("tabRelabelled:{tab}"),
        Change::TabRemoved(tab) => format!("tabRemoved:{tab}"),
        Change::LayoutChanged(tab) => format!("layoutChanged:{tab}"),
        Change::Restored(restored) => format!(
            "restored:lost=[{}]:saving={}",
            restored
                .lost_tabs
                .iter()
                .map(ToString::to_string)
                .chain(restored.lost_panes.iter().map(ToString::to_string))
                .collect::<Vec<_>>()
                .join(","),
            if restored.saving_stopped { "stopped" } else { "on" }
        ),
        Change::Restarted(restart) => format!(
            "restarted:again={}:lost=[{}]:agents=[{}]",
            restart.started_again,
            restart.lost.iter().map(|(pane, _)| pane.to_string()).collect::<Vec<_>>().join(","),
            restart
                .agents
                .iter()
                .map(|(pane, _, _)| pane.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
        Change::PasteHeld { pane, text } => format!("pasteHeld:{pane}:{}", text.len()),
        Change::ClipboardWrite { pane, text } => format!("clipboardWrite:{pane}:{}", text.len()),
        Change::Rang(pane) => format!("rang:{pane}"),
        Change::Notified { pane, title, body } => format!("notified:{pane}:{title}:{body}"),
        Change::ProgressChanged(pane) => format!("progress:{pane}"),
        Change::HumanNoticed(group) => format!("humanNoticed:{group}"),
    }
}

fn read_event(given: &Value) -> BackendEvent {
    let names = |key: &str| -> Vec<String> {
        given
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    match text(given, "kind").as_str() {
        "paneOpened" => BackendEvent::PaneOpened(read_pane(given)),
        "paneChanged" => BackendEvent::PaneChanged(read_pane(given)),
        "paneClosed" => BackendEvent::PaneClosed(PaneId::new(text(given, "id"))),
        "tabOpened" => BackendEvent::TabOpened(read_tab(given)),
        "tabChanged" => BackendEvent::TabChanged(read_tab(given)),
        "tabClosed" => BackendEvent::TabClosed(TabId::new(text(given, "id"))),
        "restored" => BackendEvent::Restored(Restored {
            lost_tabs: names("lostTabs").into_iter().map(TabId::new).collect(),
            lost_panes: names("lostPanes").into_iter().map(PaneId::new).collect(),
            saving_stopped: given.get("savingStopped").and_then(Value::as_bool).unwrap_or(false),
        }),
        "pasteHeld" => BackendEvent::PasteHeld {
            pane: PaneId::new(text(given, "pane")),
            text: text(given, "text"),
        },
        "clipboardWrite" => BackendEvent::ClipboardWrite {
            pane: PaneId::new(text(given, "pane")),
            text: text(given, "text"),
        },
        "bell" => BackendEvent::Bell { pane: PaneId::new(text(given, "pane")) },
        "notified" => BackendEvent::Notified {
            pane: PaneId::new(text(given, "pane")),
            title: text(given, "title"),
            body: text(given, "body"),
        },
        "progress" => BackendEvent::Progress {
            pane: PaneId::new(text(given, "pane")),
            progress: given.get("state").and_then(Value::as_str).map(|state| Progress {
                state: match state {
                    "running" => ProgressState::Running,
                    "error" => ProgressState::Error,
                    "indeterminate" => ProgressState::Indeterminate,
                    "paused" => ProgressState::Paused,
                    other => panic!("corpus case names a progress state nobody reads: {other:?}"),
                },
                percent: given
                    .get("percent")
                    .and_then(Value::as_u64)
                    .and_then(|percent| u8::try_from(percent).ok()),
            }),
        },
        "humanNotice" => BackendEvent::HumanNotice {
            group: text(given, "group"),
            notice: read_human_notice(given),
        },
        // Loudly, because a case naming an event this driver cannot build would otherwise pass
        // by exercising nothing at all.
        other => panic!("corpus case names an event kind the driver does not know: {other:?}"),
    }
}
