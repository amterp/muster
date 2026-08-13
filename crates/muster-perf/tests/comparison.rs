//! This is the code that decides whether a build fails, so its own failure modes are the
//! expensive ones: a regression that reads as clean, or a benchmark that stopped running
//! and looks like one that passed.

use muster_perf::{Baseline, Cost, compare, pending, table, verdict};

fn cost(name: &str, best: f64) -> Cost {
    cost_in(name, best, "ns/byte")
}

fn cost_in(name: &str, best: f64, unit: &str) -> Cost {
    Cost {
        name: name.to_string(),
        unit: unit.to_string(),
        best,
        median: best * 1.1,
        iterations: 50,
    }
}

fn baseline(costs: Vec<Cost>) -> Baseline {
    Baseline { recorded: "2026-08-13T00:00:00Z".to_string(), machine: "test".to_string(), costs }
}

#[test]
fn a_cost_that_doubled_past_the_tolerance_fails() {
    let comparison =
        compare(&[cost("frame.decode", 4.2)], &baseline(vec![cost("frame.decode", 2.0)]), 2.0);

    assert!(!comparison.is_clean());
    assert_eq!(
        comparison.regressions.iter().map(|r| &r.name).collect::<Vec<_>>(),
        ["frame.decode"]
    );
    assert!((comparison.regressions[0].ratio() - 2.1).abs() < 1e-9);
}

#[test]
fn drift_inside_the_tolerance_is_not_a_failure() {
    // The boundary matters: a gate that fires at exactly the tolerance is a gate whose
    // documented threshold is a lie, and this one is loose precisely so it never cries wolf.
    let comparison = compare(
        &[cost("frame.decode", 4.0), cost("input.encode", 1.0)],
        &baseline(vec![cost("frame.decode", 2.0), cost("input.encode", 1.9)]),
        2.0,
    );

    assert!(comparison.is_clean());
    assert!(comparison.regressions.is_empty());
}

#[test]
fn a_benchmark_that_stopped_running_is_a_failure_not_a_silent_pass() {
    // cmux shipped CI that passed with every test skipped. A benchmark dropped from the run
    // produces no number to exceed anything, so absence has to be the loud case.
    let comparison = compare(
        &[cost("frame.decode", 2.0)],
        &baseline(vec![cost("frame.decode", 2.0), cost("frame.vt_parse", 9.0)]),
        2.0,
    );

    assert!(!comparison.is_clean());
    assert_eq!(comparison.missing, ["frame.vt_parse"]);
    assert!(comparison.regressions.is_empty());
}

#[test]
fn a_new_benchmark_is_reported_but_does_not_fail_the_run() {
    // Adding coverage must not require re-recording first, or nobody adds coverage.
    let comparison = compare(
        &[cost("frame.decode", 2.0), cost_in("mirror.apply", 700.0, "ns/event")],
        &baseline(vec![cost("frame.decode", 2.0)]),
        2.0,
    );

    assert!(comparison.is_clean());
    assert_eq!(comparison.unbaselined, ["mirror.apply"]);
}

#[test]
fn a_regression_says_what_regressed_by_how_much_and_against_what() {
    let comparison = compare(
        &[cost("frame.vt_parse", 40.0)],
        &baseline(vec![cost("frame.vt_parse", 10.0)]),
        2.0,
    );

    let verdict = verdict(&comparison, 2.0);

    assert!(verdict.contains("frame.vt_parse"), "{verdict}");
    assert!(verdict.contains("ns/byte"), "{verdict}");
    assert!(verdict.contains("4.00x"), "{verdict}");
    assert!(!verdict.contains("within budget"), "{verdict}");
}

#[test]
fn a_clean_run_says_so_rather_than_saying_nothing() {
    let comparison =
        compare(&[cost("frame.decode", 2.0)], &baseline(vec![cost("frame.decode", 2.0)]), 2.0);

    assert_eq!(verdict(&comparison, 2.0), "within budget");
}

#[test]
fn the_table_carries_every_cost_and_its_unit() {
    let table = table(&[cost("frame.decode", 1.5), cost_in("input.encode", 820.0, "ns/key")]);

    assert!(table.contains("frame.decode"), "{table}");
    assert!(table.contains("input.encode"), "{table}");
    assert!(table.contains("ns/key"), "{table}");
    assert_eq!(table.lines().count(), 3, "{table}");
}

#[test]
fn a_baseline_survives_a_round_trip_through_its_file_format() {
    let original = baseline(vec![cost("frame.decode", 1.25), cost("input.encode", 820.0)]);
    let json = serde_json::to_string(&original).expect("a baseline encodes");
    let decoded: Baseline = serde_json::from_str(&json).expect("and decodes");

    // Everything but the numbers has to come back exactly. A dropped field, a renamed key
    // or a lost unit would make a baseline compare against the wrong thing, which is worse
    // than having no baseline.
    assert_eq!(decoded.recorded, original.recorded);
    assert_eq!(decoded.machine, original.machine);
    assert_eq!(decoded.costs.len(), original.costs.len());

    // The numbers come back to within a bit. serde_json writes an f64 exactly and parses it
    // back one ULP out for some values - measured at a relative error of 1.8e-16 across the
    // shapes this file holds. That is far below anything this format promises: the gate
    // compares at a 2x tolerance and the report renders two decimals, so a last-bit shift
    // cannot change a verdict. Asserting exact equality here would be asserting a property
    // of serde_json rather than of the baseline.
    for (decoded, original) in decoded.costs.iter().zip(&original.costs) {
        assert_eq!(decoded.name, original.name);
        assert_eq!(decoded.unit, original.unit);
        assert_eq!(decoded.iterations, original.iterations);
        assert!((decoded.best - original.best).abs() <= original.best * 1e-15);
        assert!((decoded.median - original.median).abs() <= original.median * 1e-15);
    }
}

#[test]
fn a_budget_with_no_code_behind_it_is_named_not_omitted() {
    // The whole point: an unmeasured budget must not look like a met one.
    let text = pending(&[
        ("mirror.apply (ns/event)", "lands with the mirror"),
        ("render at 15 panes", "needs splits"),
    ]);

    assert!(text.contains("mirror.apply (ns/event)"), "{text}");
    assert!(text.contains("render at 15 panes"), "{text}");
    assert!(text.contains("needs splits"), "{text}");
}

#[test]
fn nothing_pending_prints_nothing() {
    assert!(pending(&[]).is_empty());
}
