//! Muster's own names for panes and tabs. Cases live in corpus/conformance/pane-names.json and
//! corpus/conformance/tab-names.json.
//!
//! The corpus pins the spelling a seed produces, because a name that changed shape between
//! versions would strand every pane that already carries one in its environment. The
//! properties a name has to have - and which no single spelling can state - are asserted
//! natively below.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use conformance::{CaseError, Conformance, fields};
use muster_core::names::{Mint, Minter};
use serde_json::{Value, json};

#[test]
fn pane_names_conformance() {
    run("pane-names.json");
}

#[test]
fn tab_names_conformance() {
    run("tab-names.json");
}

fn run(file: &str) {
    let corpus = Conformance::load(file);
    let ran = corpus.run(|given| {
        let mut minter = Minter::new(mint(given)?);
        let mut trace = Vec::new();
        for step in given.get("do").and_then(Value::as_array).into_iter().flatten() {
            match step.get("draw").and_then(Value::as_str) {
                Some("pane") => trace.push(minter.pane().to_string()),
                Some("tab") => trace.push(minter.tab().to_string()),
                _ => return Err(CaseError::new(format!("a step that draws nothing: {step}"))),
            }
        }
        Ok(fields([("trace", Some(json!(trace)))]))
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

    let mut minter = Minter::new(Mint::Drawn);
    let mut seen = BTreeSet::new();
    for _ in 0..200 {
        let spelling = minter.pane().to_string();

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
        Minter::new(Mint::Replayed { at: UNIX_EPOCH + Duration::from_secs(unix_seconds), seed: 7 })
            .pane()
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

/// A minter never hands out one name twice, even when its entropy repeats.
///
/// Two panes born believing the same thing about themselves would have every later command
/// from one of them act on the other.
#[test]
fn a_name_is_never_drawn_twice() {
    let mut minter = Minter::new(replayed(4));
    let mut drawn = BTreeSet::new();
    for _ in 0..100 {
        let name = minter.pane().to_string();
        assert!(drawn.insert(name.clone()), "{name} was drawn twice");
    }
}

fn mint(given: &Value) -> Result<Mint, CaseError> {
    match given.get("mint").and_then(Value::as_str) {
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
