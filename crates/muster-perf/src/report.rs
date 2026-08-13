//! What the harness prints.
//!
//! Rendering lives here rather than in the binary for the usual reason: a report is read
//! exactly when someone is deciding whether a change is safe to ship, and one that silently
//! drops a row or mislabels a unit misinforms at the worst moment.

use std::fmt::Write as _;

use crate::cost::{Comparison, Cost};

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

/// The verdict, written so that a failure says what to do about it.
pub fn verdict(comparison: &Comparison, tolerance: f64) -> String {
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
            number(tolerance),
        ));
    }
    for name in &comparison.missing {
        lines.push(format!(
            "MISSING {name}: in the baseline but not measured. Either the benchmark was \
             removed - re-record the baseline - or it stopped running, which reads as a pass \
             and is not one."
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
