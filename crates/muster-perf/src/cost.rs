//! What a measurement is, and how a run is judged against a recorded one.

use serde::{Deserialize, Serialize};

/// The shortest iteration this harness will judge, in nanoseconds.
///
/// Below this a measurement is mostly the clock rather than the code. `pane.encoder` was
/// recorded at a single microsecond for all fifteen panes, which is what the old clock could
/// resolve and nothing about the encoder, and a 2.00x tolerance on a number that steps by
/// 100% gates nothing. A thousand nanoseconds is roughly a thousand ticks of the clock this
/// harness now uses, which is enough for the fastest sample to mean something.
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
    /// How many bytes, keys or events one iteration processed.
    ///
    /// A rate divides this out, which is exactly what makes a rate comparable - and also what
    /// hides a workload that moved underneath it. `frame.decode` replays whatever recorded
    /// frames are in the corpus, and the corpus grew by a third between the baseline being
    /// written and it next being judged, with nothing saying so. Recorded so that the next run
    /// can notice.
    ///
    /// Optional so a baseline written before this existed still parses. A missing value means
    /// "the run that wrote this did not say", not zero.
    #[serde(default)]
    pub units: Option<usize>,
    /// How long the fastest iteration took, before it was divided by `units`.
    ///
    /// Kept because `best` alone cannot say whether the measurement was long enough to time.
    #[serde(default)]
    pub nanos: Option<f64>,
    /// How many times this cost may grow before it counts as a regression.
    ///
    /// Per cost rather than one number for the file, because these benchmarks are not equally
    /// noisy and pretending otherwise makes the tolerance wrong for all of them at once.
    /// `pane.channel` binds fifteen unix sockets and spawns fifteen reader threads per
    /// iteration; `frame.decode` is arithmetic over a fixed byte array. Absent means "whatever
    /// this run was told to use".
    #[serde(default)]
    pub tolerance: Option<f64>,
}

impl Cost {
    /// Whether the fastest sample was long enough for the clock to describe.
    pub fn is_resolvable(&self) -> bool {
        self.nanos.is_none_or(|nanos| nanos >= RESOLVABLE_NANOS)
    }
}

/// What the machine was doing while a run happened.
///
/// The baseline has always recorded which machine it was measured on and never whether that
/// machine was quiet, which turns out to be the variable that matters: this is a ten-core
/// laptop with six performance cores, and past six runnable threads the scheduler starts
/// putting work on the slower four. Sampled by `./dev` rather than here, because what a load
/// average is and how you read one is a per-OS question.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Load {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    pub cores: usize,
    /// The cores that run at full speed. Equal to `cores` where the OS does not split them.
    pub fast_cores: usize,
}

/// The costs a previous run recorded, and the thing a new run is judged against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub recorded: String,
    /// What it was measured on. A baseline from another machine explains a failure that
    /// would otherwise look like a regression.
    pub machine: String,
    /// What that machine was doing at the time, when the run was told. Optional so a baseline
    /// written before this existed still parses - and a baseline that does not say is itself
    /// worth reporting, since it is the state the tier spent its first week in.
    #[serde(default)]
    pub load: Option<Load>,
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
    pub tolerance: f64,
}

impl Regression {
    pub fn ratio(&self) -> f64 {
        if self.baseline > 0.0 { self.measured / self.baseline } else { f64::INFINITY }
    }
}

/// A cost whose workload is not the one its baseline number describes.
#[derive(Debug, Clone, PartialEq)]
pub struct Restated {
    pub name: String,
    pub unit: String,
    pub baseline_units: usize,
    pub measured_units: usize,
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
    /// Measured over a different amount of work than the baseline was. Reported rather than
    /// failed, for the same reason `unbaselined` is: the fix is to re-record, and a tier that
    /// refuses to run until somebody does is a tier they stop running.
    pub restated: Vec<Restated>,
    /// Too short for the clock to describe, so neither a pass nor a failure means anything.
    pub unresolvable: Vec<String>,
}

impl Comparison {
    pub fn is_clean(&self) -> bool {
        self.regressions.is_empty() && self.missing.is_empty()
    }
}

/// Judges a run against a baseline.
///
/// `tolerance` is the fallback for a cost whose baseline entry does not carry one of its own:
/// how many times the recorded cost is still acceptable. Loose on purpose. A timing gate
/// tight enough to catch a 5% drift fires on a busy laptop instead, and a gate that cries
/// wolf gets ignored - which leaves less protection than having no gate at all. This catches
/// the regressions that matter: an accidental quadratic, a copy per byte, a per-keystroke
/// allocation storm.
///
/// Three things stop a cost being judged at all rather than judging it wrongly: no baseline
/// entry, a workload that moved, and a sample too short to time. Each is reported by name.
pub fn compare(measured: &[Cost], baseline: &Baseline, tolerance: f64) -> Comparison {
    let mut regressions = Vec::new();
    let mut unbaselined = Vec::new();
    let mut restated = Vec::new();
    let mut unresolvable = Vec::new();

    for cost in measured {
        let Some(recorded) = baseline.cost(&cost.name) else {
            unbaselined.push(cost.name.clone());
            continue;
        };
        if let (Some(was), Some(now)) = (recorded.units, cost.units)
            && was != now
        {
            restated.push(Restated {
                name: cost.name.clone(),
                unit: cost.unit.clone(),
                baseline_units: was,
                measured_units: now,
            });
            continue;
        }
        if !cost.is_resolvable() {
            unresolvable.push(cost.name.clone());
            continue;
        }
        let allowed = recorded.tolerance.unwrap_or(tolerance);
        if cost.best > recorded.best * allowed {
            regressions.push(Regression {
                name: cost.name.clone(),
                unit: cost.unit.clone(),
                baseline: recorded.best,
                measured: cost.best,
                tolerance: allowed,
            });
        }
    }

    let missing = baseline
        .costs
        .iter()
        .map(|cost| cost.name.clone())
        .filter(|name| !measured.iter().any(|cost| cost.name == *name))
        .collect();

    Comparison { regressions, unbaselined, missing, restated, unresolvable }
}
