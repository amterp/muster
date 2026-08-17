//! The mirror is the core's picture of daemon truth, and everything above it renders from
//! that picture. Cases and their reasoning live in corpus/conformance/mirror.json.

mod support;

use conformance::{Conformance, fields};
use muster_core::AgentState;
use muster_core::intent::SettledLayout;
use muster_core::mirror::backend::{Focus, PaneId, Tab, TabId, Workspace, WorkspaceId};
use muster_core::mirror::{BackendEvent, Change, Mirror};
use serde_json::{Value, json};
use support::backend::{optional, read_layout, read_pane, read_snapshot, text};

#[test]
fn mirror_conformance() {
    let corpus = Conformance::load("mirror.json");

    let ran = corpus.run(|given| {
        let mut mirror = Mirror::new();
        let mut changes = Vec::new();

        // Snapshot, then stream, then snapshot again - the order a real connection takes,
        // so that a case about a reconnect is a case about what the mirror had been told
        // before the drop rather than about a bare pair of snapshots.
        if let Some(snapshot) = given.get("snapshot") {
            mirror.bootstrap(read_snapshot(snapshot));
        }
        for step in given.get("events").and_then(Value::as_array).into_iter().flatten() {
            // An answer, in the same list as the events, because where in a stream it lands is
            // the whole question: a daemon replies to a request faster than it broadcasts what
            // the request did, so an answer taken here is ahead of the events after it.
            if text(step, "kind") == "settled" {
                changes.extend(mirror.settle(SettledLayout {
                    layout: read_layout(step),
                    stale: step.get("stale").map(read_layout),
                }));
                continue;
            }
            // The other answer that lands in a stream rather than arriving on one, and the one
            // with no event behind it at all: a backend announces a rename to nobody, so where
            // this falls among the events is the whole question. An absent `name` is a name
            // taken away, which is a real thing to ask for.
            if text(step, "kind") == "renamed" {
                changes.extend(
                    mirror.rename(&PaneId::new(text(step, "pane")), optional(step, "name")),
                );
                continue;
            }
            changes.extend(mirror.apply(read_event(step)));
        }
        // After the stream, because that is when it happens: a watcher subscribes, then asks
        // what it may already have missed. `expected` is absent when the caller believed the
        // pane had no state at all, which is different from believing it was `unknown`.
        for seed in given.get("seeds").and_then(Value::as_array).into_iter().flatten() {
            changes.extend(mirror.seed_agent_state(
                &PaneId::new(text(seed, "pane")),
                AgentState::from_backend(&text(seed, "state")),
                optional(seed, "expected").map(|state| AgentState::from_backend(&state)),
            ));
        }
        if let Some(resnapshot) = given.get("resnapshot") {
            changes.extend(mirror.bootstrap(read_snapshot(resnapshot)));
        }

        // Every field, every case. The corpus compares whole objects, and for a state
        // machine that is the point: a case asserting only what it is about would miss a
        // change that also clobbered focus, which is exactly the bug this file caught.
        Ok(fields([
            ("panes", Some(json!(ids(mirror.panes().map(|p| p.id.as_str()))))),
            ("tabs", Some(json!(ids(mirror.tabs().map(|t| t.id.as_str()))))),
            ("workspaces", Some(json!(ids(mirror.workspaces().map(|w| w.id.as_str()))))),
            ("agentStates", Some(agent_states(&mirror))),
            ("names", named(&mirror)),
            ("tabLabels", tab_labels(&mirror)),
            ("layouts", Some(layouts(&mirror))),
            ("focus", Some(focus(mirror.focus()))),
            ("health", Some(json!(mirror.health().as_str()))),
            ("changes", Some(json!(changes.iter().map(describe).collect::<Vec<_>>()))),
        ]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

fn ids<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    values.map(str::to_string).collect()
}

/// Only the cursors that point at something.
///
/// An unset cursor is absent rather than null, so a case about panes does not carry three
/// nulls describing focus it is not asserting anything about.
fn focus(focus: &Focus) -> Value {
    fields([
        ("workspace", focus.workspace.as_ref().map(|id| json!(id.as_str()))),
        ("tab", focus.tab.as_ref().map(|id| json!(id.as_str()))),
        ("pane", focus.pane.as_ref().map(|id| json!(id.as_str()))),
    ])
}

/// The name somebody gave a pane and the title its program set, for the panes that have
/// either.
///
/// Absent when no pane in the case has one, on the same reasoning as `focus` above: most
/// cases here are about structure and would otherwise carry a map of empty objects.
fn named(mirror: &Mirror) -> Option<Value> {
    let mut map = serde_json::Map::new();
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

/// What each tab is called, for the cases that are about that.
///
/// Absent when no tab has a label, on the same reasoning as `focus` above: most cases here
/// are about structure and would otherwise carry a map of empty strings.
fn tab_labels(mirror: &Mirror) -> Option<Value> {
    let mut map = serde_json::Map::new();
    for tab in mirror.tabs() {
        if !tab.label.is_empty() {
            map.insert(tab.id.to_string(), json!(tab.label));
        }
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

fn agent_states(mirror: &Mirror) -> Value {
    let mut map = serde_json::Map::new();
    for pane in mirror.panes() {
        map.insert(pane.id.to_string(), json!(pane.agent_state.as_str()));
    }
    Value::Object(map)
}

/// Each tab's tree on one line, keyed by tab.
fn layouts(mirror: &Mirror) -> Value {
    let mut map = serde_json::Map::new();
    for layout in mirror.layouts() {
        let mut described = layout.root.to_string();
        if let Some(zoomed) = &layout.zoomed {
            described = format!("{described} zoomed={zoomed}");
        }
        map.insert(layout.tab.to_string(), json!(described));
    }
    Value::Object(map)
}

/// Changes render as readable strings rather than as nested objects.
///
/// A corpus is read by people deciding whether an expectation is right, and
/// `paneRemoved:w1:p4:cascaded` says what happened where a three-field object makes the
/// reader assemble it (docs/testing.md: bytes render readably).
fn describe(change: &Change) -> String {
    match change {
        Change::PaneAdded(pane) => format!("paneAdded:{pane}"),
        Change::PaneRemoved { pane, cascaded } => {
            let how = if *cascaded { "cascaded" } else { "announced" };
            format!("paneRemoved:{pane}:{how}")
        }
        Change::AgentStateChanged { pane, from, to } => {
            format!("agentStateChanged:{pane}:{}->{}", from.as_str(), to.as_str())
        }
        Change::PaneRelabelled(pane) => format!("paneRelabelled:{pane}"),
        Change::TabAdded(tab) => format!("tabAdded:{tab}"),
        Change::TabRelabelled(tab) => format!("tabRelabelled:{tab}"),
        Change::TabRemoved(tab) => format!("tabRemoved:{tab}"),
        Change::LayoutChanged(tab) => format!("layoutChanged:{tab}"),
        Change::WorkspaceAdded(workspace) => format!("workspaceAdded:{workspace}"),
        Change::WorkspaceRelabelled(workspace) => format!("workspaceRelabelled:{workspace}"),
        Change::WorkspaceRemoved(workspace) => format!("workspaceRemoved:{workspace}"),
        Change::FocusChanged => "focusChanged".to_string(),
        Change::AgentTransitionsMissed { expected, saw } => {
            format!("agentTransitionsMissed:{expected}..{saw}")
        }
    }
}

fn read_event(given: &Value) -> BackendEvent {
    match text(given, "kind").as_str() {
        "workspaceUpserted" => BackendEvent::WorkspaceUpserted(Workspace {
            id: WorkspaceId::new(text(given, "id")),
            label: text(given, "label"),
        }),
        "workspaceRemoved" => BackendEvent::WorkspaceRemoved(WorkspaceId::new(text(given, "id"))),
        "tabRenamed" => BackendEvent::TabRenamed {
            tab: TabId::new(text(given, "id")),
            label: text(given, "label"),
        },
        "tabUpserted" => BackendEvent::TabUpserted(Tab {
            id: TabId::new(text(given, "id")),
            workspace: WorkspaceId::new(text(given, "workspace")),
            label: text(given, "label"),
        }),
        "tabRemoved" => BackendEvent::TabRemoved(TabId::new(text(given, "id"))),
        "paneUpserted" => BackendEvent::PaneUpserted(read_pane(given)),
        "paneRemoved" => BackendEvent::PaneRemoved(PaneId::new(text(given, "id"))),
        "layoutUpserted" => BackendEvent::LayoutUpserted(read_layout(given)),
        "agentStateChanged" => BackendEvent::AgentStateChanged {
            pane: PaneId::new(text(given, "pane")),
            state: AgentState::from_backend(&text(given, "state")),
        },
        "agentDetected" => BackendEvent::AgentDetected {
            pane: PaneId::new(text(given, "pane")),
            agent: text(given, "agent"),
        },
        "focusMoved" => BackendEvent::FocusMoved {
            workspace: optional(given, "workspace").map(WorkspaceId::new),
            tab: optional(given, "tab").map(TabId::new),
            pane: optional(given, "pane").map(PaneId::new),
        },
        // Loudly, because a case naming an event this driver cannot build would otherwise
        // pass by exercising nothing at all.
        other => panic!("corpus case names an event kind the driver does not know: {other:?}"),
    }
}

/// The two sequences a case cannot spell: an event after a reconnect, and one arrangement
/// arriving more times than the mirror was ever going to suppress it.
///
/// Both are about the bound on suppression, which is the property that lets this work without
/// a clock. The corpus covers everything up to a reconnect; the driver applies its resnapshot
/// last, so what happens on the stream *after* one has to be written out here.
mod suppression_is_bounded {
    use muster_core::intent::SettledLayout;
    use muster_core::mirror::backend::{Layout, LayoutNode, PaneId, SplitAxis, TabId};
    use muster_core::mirror::{BackendEvent, Mirror};

    use super::support::backend::read_snapshot;
    use serde_json::json;

    fn tree(first: &str, second: &str) -> Layout {
        Layout {
            tab: TabId::new("w1:t1"),
            root: LayoutNode::Split {
                axis: SplitAxis::Columns,
                ratio: 0.5,
                first: Box::new(LayoutNode::Pane(PaneId::new(first))),
                second: Box::new(LayoutNode::Pane(PaneId::new(second))),
            },
            focused: Some(PaneId::new("w1:p1")),
            zoomed: None,
        }
    }

    fn session() -> Mirror {
        let mut mirror = Mirror::new();
        mirror.bootstrap(read_snapshot(&json!({
            "workspaces": [{ "id": "w1", "label": "tmp" }],
            "tabs": [{ "id": "w1:t1", "workspace": "w1", "label": "1" }],
            "panes": [
                { "id": "w1:p1", "tab": "w1:t1", "workspace": "w1", "agentState": "idle" },
                { "id": "w1:p2", "tab": "w1:t1", "workspace": "w1", "agentState": "idle" },
            ],
        })));
        mirror
    }

    #[test]
    fn a_reconnect_forgets_what_it_was_suppressing() {
        // A snapshot is a fresh statement of the whole world, taken after the answer this was
        // guarding against. An arming carried across one would drop a real arrangement out of
        // the stream that follows - and that stream is the only thing that will mention it
        // again, because herdr offers no replay.
        let mut mirror = session();
        let rightward = tree("w1:p1", "w1:p2");
        mirror.settle(SettledLayout {
            layout: tree("w1:p2", "w1:p1"),
            stale: Some(rightward.clone()),
        });

        mirror.bootstrap(read_snapshot(&json!({
            "workspaces": [{ "id": "w1", "label": "tmp" }],
            "tabs": [{ "id": "w1:t1", "workspace": "w1", "label": "1" }],
            "panes": [
                { "id": "w1:p1", "tab": "w1:t1", "workspace": "w1", "agentState": "idle" },
                { "id": "w1:p2", "tab": "w1:t1", "workspace": "w1", "agentState": "idle" },
            ],
        })));

        assert!(
            !mirror.apply(BackendEvent::LayoutUpserted(rightward.clone())).is_empty(),
            "a reconnected mirror is still suppressing an arrangement from before the gap"
        );
        assert_eq!(mirror.layout(&TabId::new("w1:t1")), Some(&rightward));
    }

    /// One tab at a named divider position, which is the only thing a drag changes.
    fn at(ratio: f32) -> Layout {
        let mut layout = tree("w1:p1", "w1:p2");
        if let LayoutNode::Split { ratio: held, .. } = &mut layout.root {
            *held = ratio;
        }
        layout
    }

    /// How many positions a real drag has in flight at once.
    ///
    /// About a hundred requests a second against a broadcast a hundred milliseconds behind
    /// (kan a_28h3eBJa2), so ten. The bound has to cover this or a drag lands back where the
    /// gesture began, which is what it did.
    const A_DRAG: u16 = 10;

    /// The divider positions a gesture of this length passes through, in order.
    ///
    /// Spaced so that every one is exact in an `f32` and distinct from the rest: a test whose
    /// positions rounded into each other would be asserting on float noise rather than on
    /// suppression.
    fn positions(count: u16) -> Vec<f32> {
        (1..=count).map(|step| f32::from(step) / 1024.0).collect()
    }

    #[test]
    fn a_whole_drags_worth_of_answers_is_remembered() {
        // The regression. A dragged divider is the fastest thing that produces answers ahead of
        // their own broadcasts, and every position between the answer and its broadcast has to
        // be recognisable as news already heard. A bound sized for a resize chord at key-repeat
        // speed is three times too small for this, and what that looks like is a divider
        // snapping back to where the drag started.
        let mut mirror = session();
        let dragged = positions(A_DRAG);
        for ratio in &dragged {
            mirror.settle(SettledLayout { layout: at(*ratio), stale: None });
        }

        // Every position the drag passed through, broadcast in the order herdr would.
        for ratio in &dragged[..dragged.len() - 1] {
            assert!(
                mirror.apply(BackendEvent::LayoutUpserted(at(*ratio))).is_empty(),
                "the broadcast for {ratio} was applied, so the divider jumped back to it \
                 mid-drag"
            );
        }
        assert_eq!(
            mirror.layout(&TabId::new("w1:t1")),
            Some(&at(*dragged.last().expect("a drag has positions"))),
            "the tab did not stay where the drag left it"
        );
    }

    #[test]
    fn more_answers_than_the_bound_drop_the_oldest_rather_than_growing() {
        // The list is capped, so something going faster than any gesture stops suppressing the
        // arrangement furthest behind - one frame of a divider jumping backwards. The
        // alternative is a list that grows for as long as anything keeps asking.
        //
        // Deliberately not written against the bound's own number: what has to hold is that
        // there is one, and how big it is comes from a measurement that has already moved once.
        let mut mirror = session();
        let far_past_any_gesture = positions(1_000);
        for ratio in &far_past_any_gesture {
            mirror.settle(SettledLayout { layout: at(*ratio), stale: None });
        }

        let oldest = at(far_past_any_gesture[0]);
        assert!(
            !mirror.apply(BackendEvent::LayoutUpserted(oldest.clone())).is_empty(),
            "the oldest arrangement is still being suppressed, so the list grew unbounded"
        );
        // Which puts the tab back where that broadcast said, since nothing is suppressing it.
        assert_eq!(mirror.layout(&TabId::new("w1:t1")), Some(&oldest));
    }
}

#[test]
fn exchanging_two_panes_moves_the_ids_and_leaves_the_shape() {
    // How an adapter reconstructs the arrangement its backend published between the halves of
    // a compound intent. A swap exchanges what sits in two places rather than rearranging the
    // places, so the ratios, the axes and both cursors are untouched - a cursor names a pane,
    // and a pane takes its focus with it when it moves.
    use muster_core::mirror::backend::{Layout, LayoutNode, SplitAxis};

    let settled = Layout {
        tab: TabId::new("w1:t1"),
        root: LayoutNode::Split {
            axis: SplitAxis::Columns,
            ratio: 0.25,
            first: Box::new(LayoutNode::Pane(PaneId::new("w1:p2"))),
            second: Box::new(LayoutNode::Split {
                axis: SplitAxis::Rows,
                ratio: 0.5,
                first: Box::new(LayoutNode::Pane(PaneId::new("w1:p1"))),
                second: Box::new(LayoutNode::Pane(PaneId::new("w1:p3"))),
            }),
        },
        focused: Some(PaneId::new("w1:p2")),
        zoomed: None,
    };

    let before = settled.with_panes_exchanged(&PaneId::new("w1:p1"), &PaneId::new("w1:p2"));
    assert_eq!(before.root.to_string(), "columns(w1:p1, rows(w1:p2, w1:p3@0.5)@0.25)");
    assert_eq!(before.focused, settled.focused);
    assert_eq!(
        before.with_panes_exchanged(&PaneId::new("w1:p1"), &PaneId::new("w1:p2")),
        settled,
        "exchanging the same pair twice did not come back to where it started"
    );
}
