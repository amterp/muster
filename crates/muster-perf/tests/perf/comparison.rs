//! This is the code that decides whether a build fails, so its own failure modes are the
//! expensive ones: a regression that reads as clean, or a benchmark that stopped running
//! and looks like one that passed.

use muster_perf::{Baseline, Cost, Load, compare, context, pending, table, verdict};

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
        units: Some(1_000),
        // Comfortably above the resolution floor, so that the tests which are about something
        // else are not accidentally about that.
        nanos: Some(500_000.0),
        tolerance: None,
    }
}

fn baseline(costs: Vec<Cost>) -> Baseline {
    Baseline {
        recorded: "2026-08-13T00:00:00Z".to_string(),
        machine: "test".to_string(),
        load: None,
        costs,
    }
}

fn load(one: f64) -> Load {
    Load { one, five: one, fifteen: one, cores: 10, fast_cores: 6 }
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

    let verdict = verdict(&comparison);

    assert!(verdict.contains("frame.vt_parse"), "{verdict}");
    assert!(verdict.contains("ns/byte"), "{verdict}");
    assert!(verdict.contains("4.00x"), "{verdict}");
    assert!(!verdict.contains("within budget"), "{verdict}");
}

#[test]
fn a_clean_run_says_so_rather_than_saying_nothing() {
    let comparison =
        compare(&[cost("frame.decode", 2.0)], &baseline(vec![cost("frame.decode", 2.0)]), 2.0);

    assert_eq!(verdict(&comparison), "within budget");
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
        assert_eq!(decoded.units, original.units);
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

#[test]
fn a_baseline_written_before_these_fields_existed_still_parses() {
    // The failure this guards against is silent and total: judge() reads a parse failure as
    // "no baseline" and exits 2, so one required field added here would take the tier away
    // from everybody until they re-recorded - and the tier exists to stop exactly that kind
    // of quiet loss of coverage.
    let old = r#"{
      "recorded": "2026-08-13T23:26:32.163Z",
      "machine": "arm64-Darwin 25.4.0",
      "costs": [
        { "name": "frame.decode", "unit": "ns/byte", "best": 0.83, "median": 0.86,
          "iterations": 50 }
      ]
    }"#;

    let decoded: Baseline = serde_json::from_str(old).expect("an old baseline still parses");

    assert!(decoded.load.is_none());
    assert_eq!(decoded.costs[0].units, None);
    assert_eq!(decoded.costs[0].nanos, None);
    assert_eq!(decoded.costs[0].tolerance, None);
}

#[test]
fn a_cost_carries_its_own_tolerance_rather_than_the_runs() {
    // pane.channel binds fifteen sockets and spawns fifteen threads per iteration; frame.decode
    // is arithmetic over a byte array. One number for both is wrong for both.
    let mut noisy = cost_in("pane.channel", 90_000.0, "ns/pane");
    noisy.tolerance = Some(3.0);
    let mut quiet = cost("frame.decode", 1.0);
    quiet.tolerance = Some(1.5);

    let comparison = compare(
        &[cost_in("pane.channel", 250_000.0, "ns/pane"), cost("frame.decode", 1.6)],
        &baseline(vec![noisy, quiet]),
        2.0,
    );

    // 2.78x passes at its own 3.0 and would have failed the run's 2.0; 1.60x fails at its own
    // 1.5 and would have passed it.
    assert_eq!(
        comparison.regressions.iter().map(|r| &r.name).collect::<Vec<_>>(),
        ["frame.decode"]
    );
    assert!((comparison.regressions[0].tolerance - 1.5).abs() < 1e-9);
}

#[test]
fn a_baseline_without_a_tolerance_falls_back_to_the_runs() {
    let comparison =
        compare(&[cost("frame.decode", 3.9)], &baseline(vec![cost("frame.decode", 2.0)]), 2.0);

    assert!(comparison.is_clean());
}

#[test]
fn a_workload_that_moved_is_reported_rather_than_judged() {
    // frame.decode replays whatever recorded frames are in the corpus, and the corpus grew by a
    // third between the baseline being written and it next being judged. A rate divides its
    // workload out, so the two numbers describe different work - and reading the difference as
    // a regression sends somebody hunting a change nobody made.
    let mut recorded = cost("frame.decode", 0.83);
    recorded.units = Some(146_908);
    let mut measured = cost("frame.decode", 1.84);
    measured.units = Some(197_609);

    let comparison = compare(&[measured], &baseline(vec![recorded]), 2.0);

    assert!(comparison.is_clean(), "a moved workload is not a regression");
    assert!(comparison.regressions.is_empty());
    assert_eq!(comparison.restated.iter().map(|r| &r.name).collect::<Vec<_>>(), ["frame.decode"]);

    let verdict = verdict(&comparison);
    assert!(verdict.contains("197609"), "{verdict}");
    assert!(verdict.contains("146908"), "{verdict}");
    assert!(verdict.contains("bytes"), "{verdict}");
}

#[test]
fn a_sample_too_short_to_time_gates_nothing() {
    // pane.encoder's whole iteration was one tick of a microsecond clock, so its recorded
    // 66.67 ns/pane was the clock's resolution over fifteen panes. A quantity that steps by
    // 100% cannot be judged at any tolerance, and passing it silently is the failure: it read
    // as a benchmark holding a budget for weeks while measuring nothing.
    let mut measured = cost_in("pane.encoder", 66.67, "ns/pane");
    measured.nanos = Some(900.0);

    let comparison =
        compare(&[measured], &baseline(vec![cost_in("pane.encoder", 20.0, "ns/pane")]), 2.0);

    assert!(comparison.is_clean());
    assert!(comparison.regressions.is_empty());
    assert_eq!(comparison.unresolvable, ["pane.encoder"]);
    assert!(verdict(&comparison).contains("unmeasurable pane.encoder"), "{}", verdict(&comparison));
}

#[test]
fn a_busier_machine_than_the_baseline_says_so_before_the_verdict() {
    let mut recorded = baseline(vec![cost("frame.decode", 0.83)]);
    recorded.load = Some(load(0.4));

    let notes = context(&recorded, "test", Some(load(6.1)));

    assert!(notes.contains("0.40"), "{notes}");
    assert!(notes.contains("6.10"), "{notes}");
    assert!(notes.contains("performance core"), "{notes}");
}

#[test]
fn a_baseline_that_never_recorded_the_machines_load_says_that_too() {
    // The state perf/baseline.json spent its first week in, and the reason nobody could tell a
    // busy run from a slow one.
    let notes = context(&baseline(vec![cost("frame.decode", 0.83)]), "test", Some(load(0.2)));

    assert!(notes.contains("does not record what the machine was doing"), "{notes}");
}

#[test]
fn a_quiet_run_against_a_quiet_baseline_says_nothing() {
    let mut recorded = baseline(vec![cost("frame.decode", 0.83)]);
    recorded.load = Some(load(0.4));

    assert!(context(&recorded, "test", Some(load(0.6))).is_empty());
}
