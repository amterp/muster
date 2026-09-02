//! What Muster asks herdr for, in herdr's own words. Cases live in
//! corpus/conformance/backend-intent.json.
//!
//! Three checks, and the last two are the ones that generalise. The cases pin the envelope
//! Muster builds; `every_parameter_is_one_herdr_declares` pins it against the schema herdr
//! generates from its own request types - so an intent added later gets the misspelled-key
//! check for free, without anybody remembering to write a case for it. And
//! `every_pane_a_daemon_makes_is_handed_the_users_own_config` reads the same schema from the
//! other direction: any method herdr says takes an environment is one Muster has to send one
//! on, so a fourth way of making a pane fails here rather than leaking quietly.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use conformance::{CaseError, Conformance, fields, repo_root};
use muster_core::intent::{BackendIntent, Branch, MoveDestination, Side};
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::names::{BackendPaneId, Mint, Names};
use muster_herdr::{PaneEnvironment, read_request, request};
use serde_json::{Value, json};

#[test]
fn backend_intent_conformance() {
    let corpus = Conformance::load("backend-intent.json");

    let ran = corpus.run(|given| {
        // The one case kind that is not an intent. A find changes nothing, so it is a read
        // rather than something `BackendIntent` could hold - and it is pinned here anyway,
        // because the hazard is the same one: `recent` and `recent_unwrapped` are both
        // valid values that herdr accepts, and only one of them can be scrolled to.
        let names = names(given);
        let built = match text(given, "intent")?.as_str() {
            "read" => names.backend_pane(&PaneId::new(&text(given, "pane")?)).map(|pane| {
                let rows = given.get("rows").and_then(Value::as_u64).unwrap_or_default();
                read_request(&pane, u32::try_from(rows).unwrap_or(u32::MAX))
            }),
            // The workspace a new tab goes in comes from the daemon rather than from the intent
            // (MIP-2), so a case that describes a tab says which workspace the pane it names is
            // in - the answer `HerdrBackend::workspace_for` would have fetched.
            _ => request(
                &intent(given)?,
                &pane_environment(given),
                &names,
                given.get("workspace").and_then(Value::as_str),
            ),
        };
        match built {
            Ok((method, params)) => {
                Ok(fields([("method", Some(json!(method))), ("params", Some(params))]))
            }
            // A pane Muster has no id for is a request never sent. Rendered rather than
            // propagated, because what a case about it is pinning is that nothing went out.
            Err(refusal) => Ok(fields([("refused", Some(json!(refusal.detail())))])),
        }
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// Every pane-creating request Muster sends carries the user's own herdr config path.
///
/// The gate for the leak this restore exists to close. Muster points its daemon at a config
/// file of its own with `HERDR_CONFIG_PATH`, and a pane inherits the daemon's environment - so
/// `herdr` run inside a Muster pane would read Muster's file rather than the person's own.
///
/// Read from herdr's recorded schema rather than from a list kept here, which is what makes it
/// a gate rather than a reminder: herdr declares `env` on exactly the calls that make a pane,
/// so a fifth one appearing in a pin bump has to be decided about rather than missed. The one
/// it declares and Muster does not send is named below for that reason.
#[test]
fn every_pane_a_daemon_makes_is_handed_the_users_own_config() {
    let schema = recorded_schema();
    let restoring = PaneEnvironment::restoring(&environment([("HOME", "/home/a")]));
    let expected = json!({ "HERDR_CONFIG_PATH": "/home/a/.config/herdr/config.toml" });

    let mut covered = BTreeSet::new();
    for intent in every_intent() {
        let (method, params) = request(&intent, &restoring, &backend_names(), Some("w1"))
            .expect("every id is its own name");
        let sent = params.get("env");
        let makes_a_pane = muster_herdr::makes_a_pane(&intent);
        if declared_parameters(&schema, method).is_some_and(|declared| declared.contains("env")) {
            // The same question asked of Muster's own answer, so the two cannot drift. A pane
            // Muster does not know it is making is one it never mints a name for, and that pane
            // comes up with no MUSTER_PANE - so a `muster` command run inside it is refused for
            // a pane the window is drawing.
            assert!(
                makes_a_pane,
                "herdr says `{method}` makes a pane and `makes_a_pane` does not.\n  Impact: that \
                 pane is never given a name, so nothing running in it can say which pane it is \
                 and every command from inside it is refused.\n  Fix: add that intent to \
                 `makes_a_pane` in crates/muster-herdr/src/intent.rs."
            );
            assert_eq!(
                sent,
                Some(&expected),
                "`{method}` makes a pane and was sent {sent:?} rather than the user's own herdr \
                 config path.\n  Impact: `herdr` run inside that pane reads the config file \
                 Muster wrote for its daemon instead of the person's own, which looks like \
                 nothing at all until somebody wonders why their settings stopped applying.\n  \
                 Fix: add the env to that arm of `request` in crates/muster-herdr/src/intent.rs."
            );
            covered.insert(method.to_string());
        } else {
            assert_eq!(
                sent, None,
                "`{method}` makes no pane and was sent an environment anyway.\n  Impact: herdr \
                 ignores a parameter it does not declare, so this is dead weight on the wire \
                 today and a silent behaviour change the day it declares one."
            );
            assert!(
                !makes_a_pane,
                "`{method}` makes no pane by herdr's own schema and `makes_a_pane` says it \
                 does.\n  Impact: a name is minted and put in an environment nothing sends, so \
                 the registry fills with names for panes that were never made."
            );
        }
    }

    let declares_env = methods_declaring(&schema, "env");
    let unsent: Vec<&String> = declares_env.difference(&covered).collect();
    assert_eq!(
        unsent,
        vec!["plugin.pane.open"],
        "the methods herdr says take an environment are no longer the ones Muster covers plus \
         `plugin.pane.open`.\n  Impact: a way of making a pane that carries no restore leaks \
         Muster's daemon config into that pane, and a method that stopped taking one means \
         this check is now asserting nothing.\n  Check: what moved in \
         corpus/herdr-<version>/api-schema.json since the pin was last bumped. Muster sends no \
         plugin calls, which is why that one is expected here rather than covered."
    );
}

/// The pane environment a case asks for: a restore derived from the environment it names, or
/// nothing at all.
///
/// The launching environment rather than the answer, so a case pins the derivation too - which
/// file a pane is pointed back at is exactly the thing that would be wrong in silence.
fn pane_environment(given: &Value) -> PaneEnvironment {
    match given.get("env").and_then(Value::as_object) {
        None => PaneEnvironment::none(),
        Some(named) => PaneEnvironment::restoring(
            &named
                .iter()
                .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect(),
        ),
    }
}

fn environment<const N: usize>(named: [(&str, &str); N]) -> BTreeMap<String, String> {
    named.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect()
}

#[test]
fn every_parameter_is_one_herdr_declares() {
    // The check the corpus cases cannot make on their own: they pin what Muster sends, and
    // this pins that herdr would recognise it. `./dev` already fails when the running daemon's
    // schema differs from the recorded one, so the chain runs from the daemon's own types to
    // the keys built here.
    let schema = recorded_schema();

    for (method, params) in every_request() {
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

/// Everything Muster puts on herdr's request socket, so the schema check covers all of it.
///
/// The intents, plus the one read: a find changes nothing, so it is not a `BackendIntent`,
/// and it would go unchecked if this walked the enum alone.
fn every_request() -> Vec<(&'static str, Value)> {
    // An environment on every one, so that `env` is a parameter the schema check actually
    // sees. Built empty, the pane-creating requests would carry no `env` key and the one
    // check that would notice herdr declaring one Muster never fills in would pass blind.
    let panes = PaneEnvironment::restoring(&environment([("HOME", "/home/a")]));
    let mut all: Vec<(&'static str, Value)> = every_intent()
        .iter()
        .map(|intent| {
            request(intent, &panes, &backend_names(), Some("w1")).expect("every id is its own name")
        })
        .collect();
    all.push(read_request(&BackendPaneId::new("p1"), 0));
    all
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
            run: None,
            name: None,
        },
        // Both kinds of split, because the second is two requests rather than one and only
        // the first of them is what `request` builds. What the pair adds up to is checked
        // against a real daemon in `split_sides.rs`.
        BackendIntent::SplitPane {
            pane: PaneId::new("p1"),
            side: Side::Left,
            ratio: Some(0.25),
            cwd: Some("/src/muster".into()),
            run: None,
            name: None,
        },
        // A split that also asks for a program and a name. Neither is a `pane.split` parameter -
        // herdr takes no command and no label there - so this is here to pin that they stay out
        // of the request rather than being sent hopefully into keys herdr would ignore.
        BackendIntent::SplitPane {
            pane: PaneId::new("p1"),
            side: Side::Down,
            ratio: None,
            cwd: None,
            run: Some("claude".into()),
            name: Some("🤖 A".into()),
        },
        BackendIntent::SendText {
            pane: PaneId::new("p1"),
            text: "read AGENTS.md".into(),
            enter: true,
        },
        BackendIntent::ClosePane { pane: PaneId::new("p1") },
        BackendIntent::ResizePane {
            pane: PaneId::new("p1"),
            direction: Side::Left,
            fraction: Some(0.1),
        },
        BackendIntent::ZoomPane { pane: PaneId::new("p1") },
        BackendIntent::FocusPane { pane: PaneId::new("p1") },
        BackendIntent::CreateTab {
            beside: PaneId::new("p1"),
            cwd: Some("/src/muster".into()),
            run: None,
            name: None,
        },
        BackendIntent::CreateWorkspace { cwd: Some("/src/muster".into()), run: None, name: None },
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
            to: MoveDestination::Beside { tab: TabId::new("t2"), after: PaneId::new("p2") },
        },
        BackendIntent::MovePane {
            pane: PaneId::new("p1"),
            to: MoveDestination::NewTab { name: Some("release".into()) },
        },
        BackendIntent::MovePane {
            pane: PaneId::new("p1"),
            to: MoveDestination::NewTab { name: None },
        },
        BackendIntent::CloseTab { tab: TabId::new("t1") },
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
            | BackendIntent::SendText { .. }
            | BackendIntent::MovePane { .. }
            | BackendIntent::CloseTab { .. } => {}
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

/// Every method herdr declares one named parameter on.
///
/// The same walk as [`declared_parameters`] from the other end: that answers "what does this
/// method take", and this answers "which methods take this". Both are needed to say that the
/// set Muster covers is the set herdr offers, minus the one deliberately left out.
fn methods_declaring(schema: &Value, parameter: &str) -> BTreeSet<String> {
    let Some(methods) = schema["schemas"]["request"]["oneOf"].as_array() else {
        return BTreeSet::new();
    };
    methods
        .iter()
        .filter_map(|entry| entry["properties"]["method"]["const"].as_str())
        .filter(|method| {
            declared_parameters(schema, method).is_some_and(|declared| declared.contains(parameter))
        })
        .map(str::to_string)
        .collect()
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
            ratio: number(given, "ratio"),
            cwd,
            run: given.get("run").and_then(Value::as_str).map(str::to_string),
            name: given.get("name").and_then(Value::as_str).map(str::to_string),
        }),
        "send_text" => Ok(BackendIntent::SendText {
            pane: PaneId::new(&text(given, "pane")?),
            text: text(given, "text")?,
            enter: given.get("enter").and_then(Value::as_bool).unwrap_or_default(),
        }),
        "close" => Ok(BackendIntent::ClosePane { pane: PaneId::new(&text(given, "pane")?) }),
        "resize" => Ok(BackendIntent::ResizePane {
            pane: PaneId::new(&text(given, "pane")?),
            direction: Side::parse(&text(given, "direction")?)
                .ok_or_else(|| CaseError::new("that is not a direction"))?,
            fraction: number(given, "fraction"),
        }),
        "zoom" => Ok(BackendIntent::ZoomPane { pane: PaneId::new(&text(given, "pane")?) }),
        "swap" => Ok(BackendIntent::SwapPanes {
            pane: PaneId::new(&text(given, "pane")?),
            with: PaneId::new(&text(given, "with")?),
        }),
        // Two destinations under one word, told apart by whether the case names a tab to land
        // in - which is how a caller says the same two things.
        "move" => Ok(BackendIntent::MovePane {
            pane: PaneId::new(&text(given, "pane")?),
            to: match given.get("tab").and_then(Value::as_str) {
                Some(tab) => MoveDestination::Beside {
                    tab: TabId::new(tab),
                    after: PaneId::new(&text(given, "after")?),
                },
                None => MoveDestination::NewTab {
                    name: given.get("name").and_then(Value::as_str).map(str::to_string),
                },
            },
        }),
        "closeTab" => Ok(BackendIntent::CloseTab { tab: TabId::new(&text(given, "tab")?) }),
        "focus" => Ok(BackendIntent::FocusPane { pane: PaneId::new(&text(given, "pane")?) }),
        "tab" => Ok(BackendIntent::CreateTab {
            beside: PaneId::new(&text(given, "pane")?),
            cwd,
            run: given.get("run").and_then(Value::as_str).map(str::to_string),
            name: given.get("name").and_then(Value::as_str).map(str::to_string),
        }),
        "workspace" => Ok(BackendIntent::CreateWorkspace {
            cwd,
            run: given.get("run").and_then(Value::as_str).map(str::to_string),
            name: given.get("name").and_then(Value::as_str).map(str::to_string),
        }),
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
                ratio: number(given, "ratio").unwrap_or_default(),
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

/// One of a case's numbers, at the width the wire carries. Through serde rather than a cast,
/// which is the same narrowing without a lint about it.
///
/// Keyed rather than fixed on `ratio`, because two different quantities used to share that one
/// key: a divider's absolute position and a resize's relative step. Reading both through one
/// helper is what let a case name a `ratio` while the intent it built called the same number
/// cells, with nothing in between to notice.
fn number(given: &Value, key: &str) -> Option<f32> {
    serde_json::from_value(given.get(key)?.clone()).ok()
}

fn text(given: &Value, key: &str) -> Result<String, CaseError> {
    given
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CaseError::new(format!("`{key}` is missing")))
}

/// The registry a case is driven through.
///
/// `Mint::Backend` by default, so a case saying `p1` sends `p1` and is about the envelope
/// rather than about the mint. A case that gives `names` is the other kind: it pins that a
/// name Muster minted leaves as the id the daemon knows, which is the whole point of the
/// registry and the one thing no other case here can see.
fn names(given: &Value) -> Names {
    let panes = given.get("names").and_then(Value::as_object);
    // An array rather than a map, unlike the panes above: a tab name is drawn rather than chosen,
    // so a case says which of the daemon's tabs have been seen and in what order, and the names
    // that come out are the ones `tab-names.json` pins for this instant and seed.
    let tabs = given.get("tabs_seen").and_then(Value::as_array);
    if panes.is_none() && tabs.is_none() {
        return backend_names();
    }
    // Reproducible rather than drawn, because a tab name cannot be bound the way a pane's is:
    // nothing reserves a tab, so the only way to bind one is to let the registry name it - and a
    // case has to be able to say in advance what it will be called. The instant and seed are
    // `tab-names.json`'s, so a name written in a case here is the same name written there.
    let names = Names::alone("local", Mint::Replayed { at: minting_at(), seed: 1 });
    for (name, backend) in panes.into_iter().flatten() {
        // Bound directly rather than reserved first: what a case here is about is the
        // translation, and how a name came to exist is `pane-names.json`'s subject.
        names.settle(&PaneId::new(name.as_str()), backend.as_str().unwrap_or_default());
    }
    for backend in tabs.into_iter().flatten().filter_map(Value::as_str) {
        names.tab(backend);
    }
    names
}

/// `2026-08-17T00:00:00Z`, the instant `tab-names.json` mints at.
// Seconds rather than the hours clippy prefers: 1786924800 is a Unix timestamp, which a reader can
// recognize and look up. 496368 hours is a number nobody can place. The same trade `names::spelling`
// makes about its epoch.
#[allow(clippy::duration_suboptimal_units)]
fn minting_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_786_924_800)
}

/// A registry whose name for a pane is the daemon's own id for it.
fn backend_names() -> Names {
    Names::alone("local", Mint::Backend)
}
