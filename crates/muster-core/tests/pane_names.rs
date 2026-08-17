//! Muster's own name for a pane. Cases live in corpus/conformance/pane-names.json.
//!
//! The corpus pins the sequence a seed produces, because a name that changed shape between
//! versions would strand every pane that already carries one in its environment. The
//! properties a name has to have - and which no single sequence can state - are asserted
//! natively below.

use std::collections::BTreeSet;

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::PaneId;
use muster_core::names::{BackendPaneId, Mint, PaneNames, from_toml, to_toml};
use serde_json::{Map, Value, json};

#[test]
fn pane_names_conformance() {
    let corpus = Conformance::load("pane-names.json");

    let ran =
        corpus.run(|given| {
            let mut names = PaneNames::new(mint(given)?);
            let mut trace: Vec<String> = Vec::new();
            let mut labelled: Map<String, Value> = Map::new();

            for step in given.get("do").and_then(Value::as_array).into_iter().flatten() {
                if let Some(at) = step.get("see").and_then(Value::as_str) {
                    let (daemon, backend) = split(at)?;
                    trace.push(names.name(&daemon, &backend).to_string());
                } else if let Some(label) = step.get("reserve").and_then(Value::as_str) {
                    let reserved = names.reserve();
                    labelled.insert(label.to_string(), json!(reserved.to_string()));
                    trace.push(reserved.to_string());
                } else if let Some(settle) = step.get("settle") {
                    let label = settle["as"].as_str().unwrap_or_default();
                    let name =
                        PaneId::new(labelled.get(label).and_then(Value::as_str).ok_or_else(
                            || CaseError::new(format!("nothing reserved as {label:?}")),
                        )?);
                    let (daemon, backend) = split(settle["at"].as_str().unwrap_or_default())?;
                    names.settle(&name, &daemon, &backend);
                } else if let Some(label) = step.get("release").and_then(Value::as_str) {
                    let name =
                        PaneId::new(labelled.get(label).and_then(Value::as_str).ok_or_else(
                            || CaseError::new(format!("nothing reserved as {label:?}")),
                        )?);
                    names.release(&name);
                } else if let Some(prune) = step.get("prune") {
                    let daemon = DaemonId::new(prune["daemon"].as_str().unwrap_or_default());
                    let holds: BTreeSet<BackendPaneId> = prune["holds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(BackendPaneId::new)
                        .collect();
                    names.prune(&daemon, &holds);
                } else {
                    return Err(CaseError::new(format!("no step this driver knows in {step}")));
                }
            }

            let located: Map<String, Value> = names
                .entries()
                .map(|(name, at)| {
                    (name.to_string(), json!(format!("{}/{}", at.daemon, at.backend)))
                })
                .collect();

            Ok(fields([("trace", Some(json!(trace))), ("located", Some(Value::Object(located)))]))
        });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// What a drawn name is allowed to be.
///
/// The corpus pins one sequence, which cannot say that *every* name has these properties -
/// and every one of them is load-bearing somewhere else. The prefix is what stops a name
/// reading as the sidebar's position number; the alphabet is what stops a name being
/// transcribed wrong off a screen; the length is what an agent copies and a log line carries.
#[test]
fn a_drawn_name_is_short_typeable_and_unmistakable() {
    const ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

    let mut names = PaneNames::new(Mint::Drawn { state: 20_260_817 });
    let mut seen = BTreeSet::new();
    for pane in 0..1000 {
        let drawn = names.name(&DaemonId::new("local"), &BackendPaneId::new(format!("w1:p{pane}")));
        let spelling = drawn.to_string();

        assert_eq!(spelling.len(), 7, "a name is `p` and six characters, and {spelling} is not");
        assert!(spelling.starts_with('p'), "a name says it is a pane, and {spelling} does not");
        assert!(
            spelling[1..].chars().all(|c| ALPHABET.contains(c)),
            "{spelling} holds a character somebody could read as another one"
        );
        assert!(seen.insert(spelling.clone()), "{spelling} was handed out twice");
    }
}

/// A name that has been handed out is not handed out again while its pane is being made.
///
/// The window is one request wide and the odds are tiny, which is exactly what makes it the
/// kind of bug nobody reproduces: two panes would be born believing the same thing about
/// themselves, and every later command from one of them would act on the other.
#[test]
fn a_reserved_name_is_not_drawn_twice() {
    let mut names = PaneNames::new(Mint::Drawn { state: 4 });
    let mut reserved = Vec::new();
    for _ in 0..100 {
        let name = names.reserve();
        assert!(!reserved.contains(&name), "{name} was reserved twice");
        reserved.push(name);
    }
}

#[test]
fn what_is_written_is_what_comes_back() {
    // The file is the only thing between one run and the next. A pane that loses its name on
    // restart is an agent that can no longer say which pane it is - and it has no way to
    // find out again, because nothing else in its environment says.
    let mut names = PaneNames::new(Mint::Drawn { state: 99 });
    let first = names.name(&DaemonId::new("local"), &BackendPaneId::new("w1:p1"));
    let second = names.name(&DaemonId::new("devenv"), &BackendPaneId::new("w1:p1"));

    let read = from_toml(&to_toml(&names), Mint::Drawn { state: 1 })
        .expect("what this wrote, it can read");

    assert_eq!(
        read.locate(&first).map(|at| at.backend.to_string()),
        Some("w1:p1".to_string()),
        "a name did not survive the file"
    );
    assert_eq!(read.locate(&first).map(|at| at.daemon.to_string()), Some("local".to_string()));
    assert_eq!(read.locate(&second).map(|at| at.daemon.to_string()), Some("devenv".to_string()));
    assert_ne!(first, second, "one backend id on two daemons is two panes");
}

/// A name read back from the file is not handed out again to a different pane.
///
/// The sharp edge of persisting them: the mint knows nothing about what a previous run drew,
/// so the check has to be against everything the registry holds rather than against this
/// run's draws.
#[test]
fn a_name_read_back_is_not_drawn_again() {
    let mut before = PaneNames::new(Mint::Drawn { state: 11 });
    let taken = before.name(&DaemonId::new("local"), &BackendPaneId::new("w1:p1"));

    // The same seed, so the next run draws the same first name - which is the collision this
    // is about, and the one a random-per-draw mint would hide rather than fix.
    let mut after =
        from_toml(&to_toml(&before), Mint::Drawn { state: 11 }).expect("it can read its own file");
    let drawn = after.name(&DaemonId::new("local"), &BackendPaneId::new("w1:p2"));

    assert_ne!(drawn, taken, "a name was handed to a second pane after being read back");
}

#[test]
fn a_file_from_a_format_nobody_knows_is_refused_by_name() {
    let refusal = from_toml("version = 99\n", Mint::Backend).expect_err("version 99 is not this");
    assert!(
        refusal.contains("version 99") && refusal.contains("made again"),
        "the refusal should name the version and say what it costs, and said: {refusal}"
    );
}

fn mint(given: &Value) -> Result<Mint, CaseError> {
    match given.get("mint").and_then(Value::as_str) {
        Some("backend") => Ok(Mint::Backend),
        Some("drawn") | None => {
            Ok(Mint::Drawn { state: given.get("seed").and_then(Value::as_u64).unwrap_or(1) })
        }
        Some(other) => Err(CaseError::new(format!("no mint called {other:?}"))),
    }
}

/// `local/w1:p1`, which is how a case spells a pane in one string.
fn split(at: &str) -> Result<(DaemonId, BackendPaneId), CaseError> {
    let (daemon, backend) = at
        .split_once('/')
        .ok_or_else(|| CaseError::new(format!("{at:?} does not name a daemon and a pane")))?;
    Ok((DaemonId::new(daemon), BackendPaneId::new(backend)))
}
