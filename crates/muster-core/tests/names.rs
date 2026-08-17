//! Muster's own names for panes and tabs. Cases live in corpus/conformance/pane-names.json and
//! corpus/conformance/tab-names.json.
//!
//! The corpus pins the sequence a seed produces, because a name that changed shape between
//! versions would strand every pane that already carries one in its environment. The
//! properties a name has to have - and which no single sequence can state - are asserted
//! natively below.
//!
//! One driver file for both nouns, because the registry is one mechanism and the date helpers at
//! the foot are the awkward part of driving it. Two corpus files rather than one, because the two
//! nouns are named for different reasons and a case should say which it is about.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use conformance::{CaseError, Conformance, fields};
use muster_core::composition::DaemonId;
use muster_core::mirror::backend::{PaneId, TabId};
use muster_core::names::{
    BackendPaneId, BackendTabId, Mint, PaneNames, TabNames, from_toml, to_toml,
};
use serde_json::{Map, Value, json};

#[test]
fn pane_names_conformance() {
    let corpus = Conformance::load("pane-names.json");

    let ran = corpus.run(|given| {
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
                let name = PaneId::new(
                    labelled
                        .get(label)
                        .and_then(Value::as_str)
                        .ok_or_else(|| CaseError::new(format!("nothing reserved as {label:?}")))?,
                );
                let (daemon, backend) = split(settle["at"].as_str().unwrap_or_default())?;
                names.settle(&name, &daemon, &backend);
            } else if let Some(label) = step.get("release").and_then(Value::as_str) {
                let name = PaneId::new(
                    labelled
                        .get(label)
                        .and_then(Value::as_str)
                        .ok_or_else(|| CaseError::new(format!("nothing reserved as {label:?}")))?,
                );
                names.release(&name);
            } else if let Some(at) = step.get("resolve").and_then(Value::as_str) {
                // `local/p1w3r07bsd` - a daemon, and a name Muster minted. The outward
                // direction, which is what every request and every CLI argument needs and
                // which the trace of minted names cannot say anything about.
                let (daemon, name) = at
                    .split_once('/')
                    .ok_or_else(|| CaseError::new(format!("{at:?} names no daemon")))?;
                let resolved = names.backend(&DaemonId::new(daemon), &PaneId::new(name));
                trace.push(
                    resolved.map_or_else(|| "(nothing)".to_string(), |backend| backend.to_string()),
                );
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
            .map(|(name, at)| (name.to_string(), json!(format!("{}/{}", at.daemon, at.backend))))
            .collect();

        Ok(fields([("trace", Some(json!(trace))), ("located", Some(Value::Object(located)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn tab_names_conformance() {
    let corpus = Conformance::load("tab-names.json");

    let ran = corpus.run(|given| {
        let mut names = TabNames::new(mint(given)?);
        let mut trace: Vec<String> = Vec::new();

        for step in given.get("do").and_then(Value::as_array).into_iter().flatten() {
            if let Some(at) = step.get("see").and_then(Value::as_str) {
                let (daemon, backend) = split(at)?;
                trace.push(names.name(&daemon, &BackendTabId::new(backend.as_str())).to_string());
            } else if let Some(at) = step.get("answered").and_then(Value::as_str) {
                // What `tab.create` produces: a tab named from a reply, before anything has
                // announced it. Its own step because the only difference is invisible until a
                // prune runs.
                let (daemon, backend) = split(at)?;
                trace.push(
                    names
                        .name_from_answer(&daemon, &BackendTabId::new(backend.as_str()))
                        .to_string(),
                );
            } else if let Some(at) = step.get("resolve").and_then(Value::as_str) {
                // `local/t1w3r07bsd` - the outward direction, which is what every request about a
                // tab needs and which the trace of minted names cannot say anything about.
                let (daemon, name) = at
                    .split_once('/')
                    .ok_or_else(|| CaseError::new(format!("{at:?} names no daemon")))?;
                let resolved = names.backend(&DaemonId::new(daemon), &TabId::new(name));
                trace.push(
                    resolved.map_or_else(|| "(nothing)".to_string(), |backend| backend.to_string()),
                );
            } else if let Some(prune) = step.get("prune") {
                let daemon = DaemonId::new(prune["daemon"].as_str().unwrap_or_default());
                let holds: BTreeSet<BackendTabId> = prune["holds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(BackendTabId::new)
                    .collect();
                names.prune(&daemon, &holds);
            } else {
                return Err(CaseError::new(format!("no step this driver knows in {step}")));
            }
        }

        let located: Map<String, Value> = names
            .entries()
            .map(|(name, at)| (name.to_string(), json!(format!("{}/{}", at.daemon, at.backend))))
            .collect();

        Ok(fields([("trace", Some(json!(trace))), ("located", Some(Value::Object(located)))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

/// What a name a running Muster mints is allowed to be.
///
/// The only test that goes through the real clock and the machine's own entropy, which is the
/// path every pane a user opens is named by and the one no case can pin. Each property is
/// load-bearing somewhere else: the prefix is what stops a name reading as the sidebar's
/// position number, the alphabet is what stops a name being transcribed wrong off a screen,
/// and the length is what an agent copies and a log line carries.
#[test]
fn a_drawn_name_is_short_typeable_and_unmistakable() {
    const ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

    let mut names = PaneNames::new(Mint::Drawn);
    let mut seen = BTreeSet::new();
    for pane in 0..200 {
        let drawn = names.name(&DaemonId::new("local"), &BackendPaneId::new(format!("w1:p{pane}")));
        let spelling = drawn.to_string();

        // Ten until 2036, when the tick count crosses 32^5 and gains a character - the test
        // below pins that date. Pinned rather than derived, because "a name is ten characters"
        // is the promise made to whoever reads one, and a derived length would hold no matter
        // what it drifted to.
        assert_eq!(spelling.len(), 10, "a name is `p` and nine characters, and {spelling} is not");
        assert!(spelling.starts_with('p'), "a name says it is a pane, and {spelling} does not");
        assert!(
            spelling[1..].chars().all(|c| ALPHABET.contains(c)),
            "{spelling} holds a character somebody could read as another one"
        );
        assert!(seen.insert(spelling.clone()), "{spelling} was handed out twice");
    }
}

/// Names sort into the order their panes were made, and stop doing so in August 2036.
///
/// The ordering is not decoration: it is what lets a window list panes in an order somebody
/// can follow, and a name in an old log be placed in time without a lookup. It holds because
/// the front of a name is a tick count and the alphabet ascends.
///
/// It stops holding when the count gains a character, because flexid does not pad it and
/// `"zzzzz" > "100000"` as strings. That is what the epoch and the tick size were chosen to
/// push out to 2036, and the second half of this test is where the date is written down: a
/// failure here in 2036 is this limit arriving, not a regression.
#[test]
fn a_pane_made_later_is_named_after_one_made_earlier() {
    let mint = |unix_seconds| {
        PaneNames::new(Mint::Replayed {
            at: UNIX_EPOCH + Duration::from_secs(unix_seconds),
            seed: 7,
        })
        .name(&DaemonId::new("local"), &BackendPaneId::new("w1:p1"))
        .to_string()
    };

    // One tick apart, then five years apart: a tick is the smallest step a name can tell
    // apart, and a working life is the span the ordering has to survive to be worth having.
    let (earlier, later) = (mint(1_786_924_800), mint(1_786_924_810));
    assert!(earlier < later, "{earlier} should sort before {later}");

    let years_later = mint(1_786_924_800 + 5 * 365 * 86_400);
    assert!(later < years_later, "{later} should sort before {years_later}");
    assert_eq!(later.len(), years_later.len(), "five years should not change a name's length");

    // 2036-08-19, a tick either side. The names still differ - only the ordering gives out.
    let (before, after) = (mint(2_102_769_910), mint(2_102_769_920));
    assert_eq!(before.len() + 1, after.len(), "the count should gain a character here");
    assert!(after < before, "and that is the boundary the ordering does not cross");
}

/// A name that has been handed out is not handed out again while its pane is being made.
///
/// The window is one request wide and the odds are tiny, which is exactly what makes it the
/// kind of bug nobody reproduces: two panes would be born believing the same thing about
/// themselves, and every later command from one of them would act on the other.
#[test]
fn a_reserved_name_is_not_drawn_twice() {
    let mut names = PaneNames::new(replayed(4));
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
    // find out again, because nothing else in its environment says. A tab that loses its name
    // is a region the saved arrangement can no longer find, so the window opens fresh.
    let mut names = PaneNames::new(replayed(99));
    let first = names.name(&DaemonId::new("local"), &BackendPaneId::new("w1:p1"));
    let second = names.name(&DaemonId::new("devenv"), &BackendPaneId::new("w1:p1"));
    let mut tabs = TabNames::new(replayed(99));
    let tab = tabs.name(&DaemonId::new("local"), &BackendTabId::new("w1:t1"));

    let (read, read_tabs) =
        from_toml(&to_toml(&names, &tabs), replayed(1)).expect("what this wrote, it can read");

    assert_eq!(
        read.locate(&first).map(|at| at.backend.to_string()),
        Some("w1:p1".to_string()),
        "a name did not survive the file"
    );
    assert_eq!(read.locate(&first).map(|at| at.daemon.to_string()), Some("local".to_string()));
    assert_eq!(read.locate(&second).map(|at| at.daemon.to_string()), Some("devenv".to_string()));
    assert_ne!(first, second, "one backend id on two daemons is two panes");

    assert_eq!(
        read_tabs.locate(&tab).map(|at| at.backend.to_string()),
        Some("w1:t1".to_string()),
        "a tab name did not survive the file"
    );
    assert_ne!(tab.to_string(), first.to_string(), "a tab and a pane never share a name");
}

/// A name read back from the file is not handed out again to a different pane.
///
/// The sharp edge of persisting them: the mint knows nothing about what a previous run drew,
/// so the check has to be against everything the registry holds rather than against this
/// run's draws.
#[test]
fn a_name_read_back_is_not_drawn_again() {
    let mut before = PaneNames::new(replayed(11));
    let taken = before.name(&DaemonId::new("local"), &BackendPaneId::new("w1:p1"));

    // The same instant and the same seed, so the next run draws the same first name - which is
    // the collision this is about, and the one a mint nobody could replay would hide rather
    // than fix.
    let (mut after, _) = from_toml(&to_toml(&before, &TabNames::default()), replayed(11))
        .expect("it can read its own file");
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
        Some("replayed") | None => Ok(Mint::Replayed {
            at: instant(given.get("at").and_then(Value::as_str).unwrap_or(DEFAULT_INSTANT))?,
            seed: given.get("seed").and_then(Value::as_u64).unwrap_or(1),
        }),
        Some(other) => Err(CaseError::new(format!("no mint called {other:?}"))),
    }
}

/// What a case that says nothing about when is minting at.
///
/// A fixed instant rather than the real clock, because a case pins the name it expects and a
/// name says what second it was minted in.
const DEFAULT_INSTANT: &str = "2026-08-17T00:00:00Z";

/// The mint the tests above use: reproducible, at the instant a case with nothing to say about
/// time is driven at.
fn replayed(seed: u64) -> Mint {
    Mint::Replayed { at: instant(DEFAULT_INSTANT).expect("the default instant reads"), seed }
}

/// `2026-08-17T00:00:00Z`, in seconds since the Unix epoch.
///
/// Hand-rolled rather than taken from a date library so that a case can say when in a form
/// somebody reads. Only the shape the corpus uses is accepted: a refusal here is a typo in a
/// case, and guessing at it would pin a name for an instant nobody wrote down.
fn instant(text: &str) -> Result<SystemTime, CaseError> {
    let refuse = || CaseError::new(format!("{text:?} is not a YYYY-MM-DDTHH:MM:SSZ instant"));
    let (date, time) = text.trim_end_matches('Z').split_once('T').ok_or_else(refuse)?;

    let mut fields = date.split('-').chain(time.split(':')).map(str::parse::<i64>);
    let mut next = || fields.next().ok_or_else(refuse)?.map_err(|_| refuse());
    let (year, month, day) = (next()?, next()?, next()?);
    let (hour, minute, second) = (next()?, next()?, next()?);

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    let seconds = u64::try_from(seconds).map_err(|_| refuse())?;
    Ok(UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Hinnant's civil-to-days, the inverse of the one the diagnostics clock formats with.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // March-based, so a leap day lands at the end of a year and the month arithmetic has no
    // special case in it.
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `local/w1:p1`, which is how a case spells a pane in one string.
fn split(at: &str) -> Result<(DaemonId, BackendPaneId), CaseError> {
    let (daemon, backend) = at.split_once('/').ok_or_else(|| {
        CaseError::new(format!("{at:?} does not name a daemon and something in it"))
    })?;
    Ok((DaemonId::new(daemon), BackendPaneId::new(backend)))
}
