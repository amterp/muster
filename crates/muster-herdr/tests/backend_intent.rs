//! What Muster asks herdr for, in herdr's own words. Cases live in
//! corpus/conformance/backend-intent.json.
//!
//! Two checks, and the second is the one that generalises. The cases pin the envelope Muster
//! builds; `every_parameter_is_one_herdr_declares` pins it against the schema herdr generates
//! from its own request types - so an intent added later gets the misspelled-key check for
//! free, without anybody remembering to write a case for it.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields, repo_root};
use muster_core::intent::{BackendIntent, Branch, Side};
use muster_core::mirror::backend::{PaneId, TabId, WorkspaceId};
use muster_herdr::request;
use serde_json::{Value, json};

#[test]
fn backend_intent_conformance() {
    // Ratios in the corpus are halves and quarters because a ratio is an f32 on the wire and a
    // double in JSON: 0.6 comes back out as 0.6000000238418579 and reads as a bug in the
    // adapter rather than as the round trip it is.
    let corpus = Conformance::load("backend-intent.json");

    let ran = corpus.run(|given| {
        let (method, params) = request(&intent(given)?);
        Ok(fields([("method", Some(json!(method))), ("params", Some(params))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn every_parameter_is_one_herdr_declares() {
    // The check the corpus cases cannot make on their own: they pin what Muster sends, and
    // this pins that herdr would recognise it. `./dev` already fails when the running daemon's
    // schema differs from the recorded one, so the chain runs from the daemon's own types to
    // the keys built here.
    let schema = recorded_schema();

    for intent in every_intent() {
        let (method, params) = request(&intent);
        let declared = declared_parameters(&schema, method).unwrap_or_else(|| {
            panic!(
                "herdr's recorded schema declares no method `{method}`.\n  Impact: this \
                 request would be refused by the daemon, so whatever it was for silently \
                 never happens.\n  Check: the method name against corpus/herdr-<version>/\
                 api-schema.json, and whether the pin moved."
            )
        });
        let sent: BTreeSet<String> =
            params.as_object().expect("params are an object").keys().cloned().collect();
        let unknown: Vec<&String> = sent.difference(&declared).collect();
        assert!(
            unknown.is_empty(),
            "`{method}` was sent {unknown:?}, which herdr does not declare.\n  Impact: herdr \
             ignores a parameter it does not know rather than refusing it, so this request \
             would act on whatever that daemon happened to have focused - and against a \
             daemon holding one workspace and one pane, that looks correct.\n  Check: the \
             parameter names in corpus/herdr-<version>/api-schema.json. `pane.split` is the \
             one that takes `target_pane_id` where the rest take `pane_id`."
        );
    }
}

/// One of every intent, so the schema check covers the whole vocabulary.
///
/// Listed rather than derived, which means a new variant has to be added here. The compiler
/// says so: `BackendIntent` is matched exhaustively below for exactly that reason.
fn every_intent() -> Vec<BackendIntent> {
    let all = vec![
        BackendIntent::SplitPane {
            pane: PaneId::new("p1"),
            side: Side::Right,
            ratio: Some(0.25),
            cwd: Some("/src/muster".into()),
        },
        // Both kinds of split, because the second is two requests rather than one and only
        // the first of them is what `request` builds. What the pair adds up to is checked
        // against a real daemon in `split_sides.rs`.
        BackendIntent::SplitPane {
            pane: PaneId::new("p1"),
            side: Side::Left,
            ratio: Some(0.25),
            cwd: Some("/src/muster".into()),
        },
        BackendIntent::ClosePane { pane: PaneId::new("p1") },
        BackendIntent::ResizePane {
            pane: PaneId::new("p1"),
            direction: Side::Left,
            amount: Some(2.0),
        },
        BackendIntent::ZoomPane { pane: PaneId::new("p1") },
        BackendIntent::FocusPane { pane: PaneId::new("p1") },
        BackendIntent::CreateTab {
            workspace: WorkspaceId::new("w1"),
            cwd: Some("/src/muster".into()),
        },
        BackendIntent::CreateWorkspace { cwd: Some("/src/muster".into()) },
        BackendIntent::SetSplitRatio {
            tab: TabId::new("t1"),
            path: vec![Branch::Second],
            ratio: 0.6,
        },
        // Both directions of both renames, because clearing is spelled differently from
        // naming and differently again between a pane and a tab - which is the whole hazard
        // this file exists to catch.
        BackendIntent::RenamePane { pane: PaneId::new("p1"), name: Some("🔥 payments".into()) },
        BackendIntent::RenamePane { pane: PaneId::new("p1"), name: None },
        BackendIntent::RenameTab { tab: TabId::new("t1"), name: Some("release".into()) },
        BackendIntent::RenameTab { tab: TabId::new("t1"), name: None },
        BackendIntent::SwapPanes { pane: PaneId::new("p1"), with: PaneId::new("p2") },
        BackendIntent::MovePane {
            pane: PaneId::new("p1"),
            tab: TabId::new("t2"),
            after: PaneId::new("p2"),
        },
    ];
    for intent in &all {
        // Exhaustive on purpose. A variant added without a line above reaches herdr with its
        // keys unchecked, and the whole point of this test is that herdr will not complain.
        match intent {
            BackendIntent::SplitPane { .. }
            | BackendIntent::ResizePane { .. }
            | BackendIntent::ZoomPane { .. }
            | BackendIntent::ClosePane { .. }
            | BackendIntent::FocusPane { .. }
            | BackendIntent::CreateTab { .. }
            | BackendIntent::CreateWorkspace { .. }
            | BackendIntent::SetSplitRatio { .. }
            | BackendIntent::RenamePane { .. }
            | BackendIntent::RenameTab { .. }
            | BackendIntent::SwapPanes { .. }
            | BackendIntent::MovePane { .. } => {}
        }
    }
    all
}

/// herdr's account of its own wire, for the version this tree is pinned to.
fn recorded_schema() -> Value {
    let root = repo_root();
    let pin = std::fs::read_to_string(root.join("deps/herdr.pin"))
        .expect("deps/herdr.pin names the daemon this suite is judged against");
    let pin: Value = serde_json::from_str(&pin).expect("deps/herdr.pin is JSON");
    let version = pin["version"].as_str().expect("deps/herdr.pin names a version");
    let path = root.join(format!("corpus/herdr-{version}/api-schema.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()));
    serde_json::from_str(&text).expect("herdr's schema is JSON")
}

/// Every parameter herdr declares for one method, or `None` when it declares no such method.
///
/// The schema is one entry per method under `schemas.request.oneOf`, each pointing its params
/// at a definition beside them.
fn declared_parameters(schema: &Value, method: &str) -> Option<BTreeSet<String>> {
    let methods = schema["schemas"]["request"]["oneOf"].as_array()?;
    let entry = methods
        .iter()
        .find(|entry| entry["properties"]["method"]["const"].as_str() == Some(method))?;
    let reference = entry["properties"]["params"]["$ref"].as_str()?;
    let name = reference.rsplit('/').next()?;
    let properties = schema["schemas"]["request"]["$defs"][name]["properties"].as_object()?;
    Some(properties.keys().cloned().collect())
}

/// One case's `given`, as the intent it describes.
fn intent(given: &Value) -> Result<BackendIntent, CaseError> {
    let kind = text(given, "intent")?;
    let cwd = given.get("cwd").and_then(Value::as_str).map(str::to_string);
    match kind.as_str() {
        "split" => Ok(BackendIntent::SplitPane {
            pane: PaneId::new(&text(given, "pane")?),
            side: Side::parse(&text(given, "side")?)
                .ok_or_else(|| CaseError::new("that is not a side"))?,
            ratio: ratio(given),
            cwd,
        }),
        "close" => Ok(BackendIntent::ClosePane { pane: PaneId::new(&text(given, "pane")?) }),
        "resize" => Ok(BackendIntent::ResizePane {
            pane: PaneId::new(&text(given, "pane")?),
            direction: Side::parse(&text(given, "direction")?)
                .ok_or_else(|| CaseError::new("that is not a direction"))?,
            amount: ratio(given),
        }),
        "zoom" => Ok(BackendIntent::ZoomPane { pane: PaneId::new(&text(given, "pane")?) }),
        "swap" => Ok(BackendIntent::SwapPanes {
            pane: PaneId::new(&text(given, "pane")?),
            with: PaneId::new(&text(given, "with")?),
        }),
        "move" => Ok(BackendIntent::MovePane {
            pane: PaneId::new(&text(given, "pane")?),
            tab: TabId::new(&text(given, "tab")?),
            after: PaneId::new(&text(given, "after")?),
        }),
        "focus" => Ok(BackendIntent::FocusPane { pane: PaneId::new(&text(given, "pane")?) }),
        "tab" => Ok(BackendIntent::CreateTab {
            workspace: WorkspaceId::new(&text(given, "workspace")?),
            cwd,
        }),
        "workspace" => Ok(BackendIntent::CreateWorkspace { cwd }),
        "ratio" => {
            let path = given
                .get("path")
                .and_then(Value::as_array)
                .ok_or_else(|| CaseError::new("`path` is missing: a divider has no other name"))?;
            let path = path
                .iter()
                .map(|turn| match turn.as_str() {
                    Some("first") => Ok(Branch::First),
                    Some("second") => Ok(Branch::Second),
                    other => Err(CaseError::new(format!("`{other:?}` is not a turn"))),
                })
                .collect::<Result<Vec<Branch>, CaseError>>()?;
            Ok(BackendIntent::SetSplitRatio {
                tab: TabId::new(&text(given, "tab")?),
                path,
                ratio: ratio(given).unwrap_or_default(),
            })
        }
        // An absent `name` is asking for the name to be taken away, which is a different
        // request from naming something and is spelled differently on the wire.
        "rename-pane" => Ok(BackendIntent::RenamePane {
            pane: PaneId::new(&text(given, "pane")?),
            name: given.get("name").and_then(Value::as_str).map(str::to_string),
        }),
        "rename-tab" => Ok(BackendIntent::RenameTab {
            tab: TabId::new(&text(given, "tab")?),
            name: given.get("name").and_then(Value::as_str).map(str::to_string),
        }),
        other => Err(CaseError::new(format!("`{other}` is not an intent this driver knows"))),
    }
}

/// A case's ratio or amount, at the width the wire carries. Through serde rather than a cast, which is
/// the same narrowing without a lint about it.
fn ratio(given: &Value) -> Option<f32> {
    serde_json::from_value(given.get("ratio")?.clone()).ok()
}

fn text(given: &Value, key: &str) -> Result<String, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CaseError::new(format!("`{key}` is missing")))
}
