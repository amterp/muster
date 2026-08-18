//! What the harness prints.
//!
//! Rendering lives here rather than in the binary for the usual reason: a report is read
//! exactly when someone is deciding whether a change is safe to ship, and one that silently
//! drops a row or mislabels a unit misinforms at the worst moment.

use std::fmt::Write as _;

use crate::cost::{Baseline, Comparison, Cost, Load, RESOLVABLE_NANOS};

pub fn table(costs: &[Cost]) -> String {
    if costs.is_empty() {
        return "no costs".to_string();
    }

    let name_width = costs.iter().map(|cost| cost.name.len()).max().unwrap_or(0).max(9);
    let unit_width = costs.iter().map(|cost| cost.unit.len()).max().unwrap_or(0).max(4);

    let mut out = format!(
        "{:name_width$}  {:unit_width$}  {:>9}  {:>9}",
        "benchmark", "unit", "best", "median"
    );
    for cost in costs {
        let _ = write!(
            out,
            "\n{:name_width$}  {:unit_width$}  {:>9}  {:>9}",
            cost.name,
            cost.unit,
            number(cost.best),
            number(cost.median)
        );
    }
    out
}

/// Budgets the desiderata name that this run cannot yet measure.
///
/// Printed rather than left out. A budget nobody wrote down reads exactly like a budget
/// nobody exceeded, which is the shape of cmux's CI passing with every test skipped
/// (docs/testing.md). Naming the gap keeps the harness honest about its own coverage.
pub fn pending(budgets: &[(&str, &str)]) -> String {
    if budgets.is_empty() {
        return String::new();
    }
    let mut out = "not measured yet:".to_string();
    for (name, why) in budgets {
        let _ = write!(out, "\n  {name} - {why}");
    }
    out
}

/// Why this run may not be comparable to its baseline at all, said before the verdict.
///
/// Both halves are the same admission: a number is only a budget if the thing that produced
/// it is the thing the budget was written for. A different machine has always been named
/// here; a busier one had not, and it is the difference that actually moved these numbers.
pub fn context(baseline: &Baseline, machine: &str, load: Option<Load>) -> String {
    let mut lines: Vec<String> = Vec::new();

    if baseline.machine != machine {
        lines.push(format!(
            "note: baseline recorded on {}, running on {machine}. A cross-machine comparison \
             explains a failure that is not a regression.",
            baseline.machine,
        ));
    }

    match (baseline.load, load) {
        (Some(then), Some(now)) if now.one > then.one + 1.0 => lines.push(format!(
            "note: the baseline was recorded at a load of {}, and this run measured at {} \
             against {} performance core(s). A benchmark that spent its time waiting for a \
             core reads slow for a reason that is not the code.",
            number(then.one),
            number(now.one),
            now.fast_cores,
        )),
        (None, Some(_)) => lines.push(
            "note: the baseline does not record what the machine was doing when it was \
             written, so nothing here can tell a busy run from a slow one. Re-recording it \
             fixes that."
                .to_string(),
        ),
        _ => {}
    }

    lines.join("\n")
}

/// The verdict, written so that a failure says what to do about it.
pub fn verdict(comparison: &Comparison) -> String {
    let mut lines: Vec<String> = Vec::new();

    let mut regressions: Vec<_> = comparison.regressions.iter().collect();
    regressions.sort_by(|a, b| b.ratio().total_cmp(&a.ratio()));
    for regression in regressions {
        lines.push(format!(
            "REGRESSED {}: {} {}, against a baseline of {} ({}x, tolerance {}x)",
            regression.name,
            number(regression.measured),
            regression.unit,
            number(regression.baseline),
            number(regression.ratio()),
            number(regression.tolerance),
        ));
    }
    for name in &comparison.missing {
        lines.push(format!(
            "MISSING {name}: in the baseline but not measured. Either the benchmark was \
             removed - re-record the baseline - or it stopped running, which reads as a pass \
             and is not one."
        ));
    }
    for restated in &comparison.restated {
        lines.push(format!(
            "changed {}: measured over {} {}, the baseline over {}. A rate divides its \
             workload out, so the two numbers describe different work and comparing them \
             would report a regression nobody caused. Re-record to gate it again.",
            restated.name,
            restated.measured_units,
            unit_noun(&restated.unit),
            restated.baseline_units,
        ));
    }
    for name in &comparison.unresolvable {
        lines.push(format!(
            "unmeasurable {name}: its fastest iteration finished in under {RESOLVABLE_NANOS:.0} \
             ns, which is close enough to the clock that the number is mostly timing overhead. \
             Give the benchmark more work per iteration; until then it gates nothing."
        ));
    }
    for name in &comparison.unbaselined {
        lines.push(format!(
            "new {name}: measured but not in the baseline, so nothing gates it yet."
        ));
    }

    match (comparison.is_clean(), lines.is_empty()) {
        (true, true) => "within budget".to_string(),
        (true, false) => lines.join("\n") + "\nwithin budget",
        (false, _) => lines.join("\n"),
    }
}

pub(crate) fn number(value: f64) -> String {
    if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// What one unit of a cost is called, for a sentence that counts them.
///
/// `ns/byte` counts bytes. Anything the harness has not been taught reads as "units", which
/// is vague but never wrong - and a benchmark whose unit is spelled some new way should not
/// produce a sentence claiming it measured bytes.
fn unit_noun(unit: &str) -> &'static str {
    match unit {
        "ns/byte" => "bytes",
        "ns/key" => "keys",
        "ns/event" => "events",
        "ns/pane" => "panes",
        _ => "units",
    }
}
