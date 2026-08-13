//! Runs a body repeatedly and reports what one unit of its work costs.

use muster_core::diagnostics::{monotonic_now, monotonic_since};

use crate::cost::Cost;

/// - `units_per_iteration`: how many bytes, keys or events one call to `body` processes.
///   The result is divided by this, so a benchmark that changes how much work it does per
///   call stays comparable to its own baseline.
/// - `warmup`: iterations whose timings are thrown away. The first pass through any of this
///   pays for lazy statics, a cold instruction cache, and in libghostty's case a dylib not
///   yet bound - none of which a running Muster pays per byte.
pub fn measure(
    name: &str,
    unit: &str,
    units_per_iteration: usize,
    iterations: usize,
    warmup: usize,
    mut body: impl FnMut(),
) -> Cost {
    assert!(units_per_iteration > 0, "{name}: a per-unit cost needs units");

    for _ in 0..warmup {
        body();
    }

    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = monotonic_now();
        body();
        #[allow(clippy::cast_precision_loss)]
        samples.push(monotonic_since(start) as f64 / units_per_iteration as f64);
    }

    samples.sort_by(f64::total_cmp);
    Cost {
        name: name.to_string(),
        unit: unit.to_string(),
        best: samples[0],
        median: samples[samples.len() / 2],
        iterations,
    }
}
