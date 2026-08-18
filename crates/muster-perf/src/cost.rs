//! What a measurement is, and how a run is judged against a recorded one.

use serde::{Deserialize, Serialize};

/// The shortest iteration this harness will judge, in nanoseconds.
///
/// Below this a measurement is mostly the clock rather than the code. A thousand nanoseconds
/// is roughly a thousand ticks of the clock this harness uses, which is enough for the fastest
/// sample to say something about the work rather than about the timing around it.
pub const RESOLVABLE_NANOS: f64 = 1_000.0;

/// One cost, measured.
///
/// Costs are always per unit of work rather than per run - nanoseconds per byte, per key,
/// per event - because the desideratum budgets by frequency times cardinality, and a
/// per-run number cannot be multiplied by anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub name: String,
    /// What one unit is: `ns/byte`, `ns/key`, `ns/event`. Part of the record because a
    /// baseline compared against a differently-scaled cost is worse than no baseline.
    pub unit: String,
    /// The gating number: the fastest per-unit cost observed.
    ///
    /// The minimum rather than the mean, because noise here is one-sided. A preempted
    /// sample is slower than the truth; nothing makes one faster. So the minimum is both
    /// the closest estimate of the real cost and the most reproducible number to compare
    /// across machines and runs.
    pub best: f64,
    /// The middle sample, kept for information. A median far above `best` means the machine
    /// was busy, which is worth seeing before trusting the run.
    pub median: f64,
    pub iterations: usize,
}

/// The costs a previous run recorded, and the thing a new run is judged against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub recorded: String,
    /// What it was measured on. A baseline from another machine explains a failure that
    /// would otherwise look like a regression.
    pub machine: String,
    pub costs: Vec<Cost>,
}

impl Baseline {
    pub fn cost(&self, name: &str) -> Option<&Cost> {
        self.costs.iter().find(|cost| cost.name == name)
    }
}

/// A cost that grew past what the baseline allows.
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub name: String,
    pub unit: String,
    pub baseline: f64,
    pub measured: f64,
}

impl Regression {
    pub fn ratio(&self) -> f64 {
        if self.baseline > 0.0 { self.measured / self.baseline } else { f64::INFINITY }
    }
}

/// How a run compares to its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub regressions: Vec<Regression>,
    /// Measured this run but absent from the baseline. Not a failure - a new benchmark is
    /// how coverage grows - but it is un-gated until someone records it.
    pub unbaselined: Vec<String>,
    /// In the baseline but not measured this run. Worth naming: a benchmark that quietly
    /// stopped running looks exactly like one that is passing.
    pub missing: Vec<String>,
}

impl Comparison {
    pub fn is_clean(&self) -> bool {
        self.regressions.is_empty() && self.missing.is_empty()
    }
}

/// Judges a run against a baseline.
///
/// `tolerance` is how many times the recorded cost is still acceptable. Loose on purpose. A
/// timing gate tight enough to catch a 5% drift fires on a busy laptop instead, and a gate
/// that cries wolf gets ignored - which leaves less protection than having no gate at all.
/// This catches the regressions that matter: an accidental quadratic, a copy per byte, a
/// per-keystroke allocation storm.
pub fn compare(measured: &[Cost], baseline: &Baseline, tolerance: f64) -> Comparison {
    let mut regressions = Vec::new();
    let mut unbaselined = Vec::new();

    for cost in measured {
        let Some(recorded) = baseline.cost(&cost.name) else {
            unbaselined.push(cost.name.clone());
            continue;
        };
        if cost.best > recorded.best * tolerance {
            regressions.push(Regression {
                name: cost.name.clone(),
                unit: cost.unit.clone(),
                baseline: recorded.best,
                measured: cost.best,
            });
        }
    }

    let missing = baseline
        .costs
        .iter()
        .map(|cost| cost.name.clone())
        .filter(|name| !measured.iter().any(|cost| cost.name == *name))
        .collect();

    Comparison { regressions, unbaselined, missing }
}
