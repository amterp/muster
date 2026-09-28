//! What herdr was measured doing, and what muster-daemon does about each part of it.
//!
//! `corpus/herdr-0.8.0/` records herdr 0.8.0 from when it was Muster's daemon, one `FACTS.json`
//! per scenario. `MIGRATION.json` beside them gives every fact one verdict: pinned by a test
//! that holds muster-daemon to the same behavior, deliberately different, or about herdr alone. This holds the verdicts to the facts: one missing, one left over, or one
//! naming a test that no longer exists fails, so the reference cannot quietly go stale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn corpus() -> PathBuf {
    Path::new(REPO).join("corpus/herdr-0.8.0")
}

/// Every scenario's facts, by scenario directory name.
fn facts() -> BTreeMap<String, Map<String, Value>> {
    let mut scenarios = BTreeMap::new();
    for entry in std::fs::read_dir(corpus()).expect("corpus/herdr-0.8.0 is there") {
        let directory = entry.expect("a readable corpus entry").path();
        let file = directory.join("FACTS.json");
        if !file.is_file() {
            continue;
        }
        let name = directory.file_name().unwrap().to_string_lossy().into_owned();
        scenarios.insert(name, object(&file));
    }
    assert!(!scenarios.is_empty(), "no FACTS.json under {}", corpus().display());
    scenarios
}

/// Every verdict, by scenario and fact.
fn verdicts() -> BTreeMap<String, Map<String, Value>> {
    let migration = object(&corpus().join("MIGRATION.json"));
    let scenarios = migration["scenarios"].as_object().expect("MIGRATION.json has `scenarios`");
    scenarios
        .iter()
        .map(|(name, facts)| {
            (name.clone(), facts.as_object().expect("a scenario's verdicts are an object").clone())
        })
        .collect()
}

fn object(path: &Path) -> Map<String, Value> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    match serde_json::from_str(&text) {
        Ok(Value::Object(object)) => object,
        other => panic!("{} is not a JSON object: {other:?}", path.display()),
    }
}

/// A verdict's kind and what it says: `pinned`, `differs` or `herdr_only`.
fn verdict(value: &Value) -> Result<(&str, &str), String> {
    let object = value.as_object().ok_or("a verdict is an object")?;
    let [(kind, said)] = object.iter().collect::<Vec<_>>()[..] else {
        return Err(format!("a verdict has exactly one kind, not {}", object.len()));
    };
    let said = said.as_str().filter(|said| !said.trim().is_empty()).ok_or("says nothing")?;
    match kind.as_str() {
        "pinned" | "differs" | "herdr_only" => Ok((kind.as_str(), said)),
        other => Err(format!("`{other}` is not a verdict")),
    }
}

#[test]
fn every_recorded_fact_has_one_verdict() {
    let facts = facts();
    let verdicts = verdicts();
    let mut wrong = Vec::new();
    for (scenario, recorded) in &facts {
        let given = verdicts.get(scenario);
        for fact in recorded.keys() {
            match given.and_then(|given| given.get(fact)) {
                None => wrong.push(format!("{scenario}/{fact}: no verdict")),
                Some(value) => {
                    if let Err(why) = verdict(value) {
                        wrong.push(format!("{scenario}/{fact}: {why}"));
                    }
                }
            }
        }
    }
    for (scenario, given) in &verdicts {
        for fact in given.keys() {
            if !facts.get(scenario).is_some_and(|recorded| recorded.contains_key(fact)) {
                wrong.push(format!("{scenario}/{fact}: a verdict for a fact nobody recorded"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "corpus/herdr-0.8.0/MIGRATION.json and the recorded facts disagree:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn every_pinned_fact_names_a_test_that_exists() {
    let mut missing = Vec::new();
    for (scenario, given) in verdicts() {
        for (fact, value) in given {
            if let Ok(("pinned", reference)) = verdict(&value)
                && let Err(why) = exists(reference)
            {
                missing.push(format!("{scenario}/{fact}: {reference}: {why}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "facts pinned by tests that are not there, so nothing holds muster-daemon to them:\n  {}",
        missing.join("\n  ")
    );
}

/// Whether `reference` names a test there is: `<file>::<name>` for a test function in Rust or
/// Swift, `<file>#<name>` for a conformance case.
fn exists(reference: &str) -> Result<(), String> {
    if let Some((file, case)) = reference.split_once('#') {
        let cases = object(&Path::new(REPO).join(file));
        let named = cases["cases"]
            .as_array()
            .is_some_and(|cases| cases.iter().any(|each| each["name"].as_str() == Some(case)));
        return if named { Ok(()) } else { Err("no case by that name".to_string()) };
    }
    let (file, name) = reference.rsplit_once("::").ok_or("neither `::` nor `#` in it")?;
    let text =
        std::fs::read_to_string(Path::new(REPO).join(file)).map_err(|error| format!("{error}"))?;
    let declared = if Path::new(file).extension().is_some_and(|extension| extension == "rs") {
        is_a_test(&text, name)
    } else {
        text.contains(&format!("func {name}(")) || text.contains(&format!("@Test(\"{name}\""))
    };
    if declared { Ok(()) } else { Err("no test by that name in it".to_string()) }
}

/// Whether `text` declares a test called `name`: a `fn` whose attributes, which may be several,
/// include `#[test]`. A helper of the same name is not a test, and a verdict pinned to one would
/// be checked by nothing.
fn is_a_test(text: &str, name: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().any(|(at, line)| {
        let line = line.trim_start();
        let declares = line.strip_prefix("fn ").or_else(|| line.strip_prefix("pub fn "));
        declares.is_some_and(|rest| rest.starts_with(&format!("{name}(")))
            && lines[..at]
                .iter()
                .rev()
                .map(|line| line.trim())
                .take_while(|line| line.starts_with("#["))
                .any(|line| line == "#[test]")
    })
}

#[test]
fn a_verdict_pinned_to_a_helper_is_pinned_to_nothing() {
    let text = "#[test]\n#[ignore]\nfn checked() {}\n\n/// A helper.\nfn helped() {}\n";
    assert!(is_a_test(text, "checked"));
    assert!(!is_a_test(text, "helped"));
    assert!(!is_a_test(text, "check"), "a prefix is a different name");
}
