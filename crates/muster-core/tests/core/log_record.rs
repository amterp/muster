//! The log is the thing that gets read when something has already gone wrong, so the ways
//! it can quietly fail are expensive: a line that will not parse, a field that leaks what
//! someone typed, a level that hides the record that mattered.
//!
//! Cases and their reasoning live in corpus/conformance/log-record.json.

use std::collections::BTreeMap;

use conformance::{CaseError, Conformance, fields};
use muster_core::diagnostics::{LogLevel, LogRecord, format_iso8601, monotonic_now, sink};
use serde_json::{Value, json};

#[test]
fn log_record_conformance() {
    let corpus = Conformance::load("log-record.json");

    let ran = corpus.run(|given| {
        let level = given
            .get("level")
            .and_then(Value::as_str)
            .and_then(LogLevel::parse)
            .ok_or_else(|| CaseError::new("`level` is missing or not a level"))?;
        let mut payload = BTreeMap::new();
        if let Some(raw) = given.get("fields").and_then(Value::as_object) {
            for (name, value) in raw {
                payload.insert(name.clone(), value.as_str().unwrap_or_default().to_string());
            }
        }
        let record = LogRecord {
            // Fixed rather than read from the case: every case uses the epoch, so the
            // rendered text is constant, and a corpus that pinned it would be testing a
            // date formatter in each language instead of this encoder.
            time_ms: 0,
            mono: given.get("mono").and_then(Value::as_u64).unwrap_or(0),
            level,
            process: given.get("process").and_then(Value::as_str).unwrap_or("").to_string(),
            pid: given
                .get("pid")
                .and_then(Value::as_i64)
                .and_then(|p| i32::try_from(p).ok())
                .unwrap_or(0),
            event: given.get("event").and_then(Value::as_str).unwrap_or("").to_string(),
            fields: payload,
        };

        Ok(fields([("line", Some(json!(sink::encode(&record))))]))
    });

    assert_eq!(ran, corpus.cases.len());
    assert!(ran > 0);
}

#[test]
fn every_encoded_record_is_parseable_json() {
    // The corpus pins the exact bytes; this pins that those bytes mean what they look
    // like. A rendering that agreed with itself in both languages and parsed in neither
    // would satisfy the cases above and still be useless.
    let corpus = Conformance::load("log-record.json");
    for case in &corpus.cases {
        let line = case.expect.get("line").and_then(Value::as_str).expect("case has no `line`");
        assert!(
            serde_json::from_str::<Value>(line).is_ok(),
            "{}: the expected line is not JSON",
            case.name
        );
    }
}

#[test]
fn the_monotonic_reading_advances_and_resolves_the_hops_we_measure() {
    // The point of carrying a second clock: `time` is milliseconds, and the hops the perf
    // harness times are tenths of one. A clock that cannot see them makes the log useless
    // as a perf oracle while still looking like it works.
    let start = monotonic_now();
    assert!(start > 0);

    let mut later = monotonic_now();
    while later == start {
        later = monotonic_now();
    }

    // Two readings taken back to back are microseconds apart at most. If this clock only
    // ticked per millisecond, the loop above would have spun for one.
    assert!(later - start < 1_000_000);
}

#[test]
fn levels_order_from_noise_to_alarm() {
    assert!(LogLevel::Trace < LogLevel::Debug);
    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
}

#[test]
fn timestamps_render_the_way_the_swift_side_renders_them() {
    // During the port both languages append to one log file. A timeline whose timestamps
    // render two ways is not a timeline, so this pins the format against known instants
    // rather than trusting that two implementations of ISO 8601 agree.
    assert_eq!(format_iso8601(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(format_iso8601(1), "1970-01-01T00:00:00.001Z");
    // A leap day, which is where a hand-rolled civil calendar goes wrong if it is wrong.
    assert_eq!(format_iso8601(1_709_164_800_000), "2024-02-29T00:00:00.000Z");
    // A century that is not a leap year, the other classic.
    assert_eq!(format_iso8601(4_107_542_400_000), "2100-03-01T00:00:00.000Z");
}
