//! Runs a body repeatedly and reports what one unit of its work costs.

use std::time::Instant;

use crate::cost::Cost;

/// - `units_per_iteration`: how many bytes, keys or events one call to `body` processes.
///   The result is divided by this, so a benchmark that changes how much work it does per
///   call stays comparable to its own baseline.
/// - `warmup`: iterations whose timings are thrown away. The first pass through any of this
///   pays for lazy statics, a cold instruction cache, and in libghostty's case a dylib not
///   yet bound - none of which a running Muster pays per byte.
///
/// Timed with `Instant` rather than with the core's `monotonic_now`, which is the one place
/// the harness departs from the shared clock. `CLOCK_MONOTONIC` is what the log uses because
/// two processes can subtract two readings of it, and that is worth a microsecond of
/// resolution there. It is not worth it here: on Darwin that clock resolves exactly 1 µs, so
/// every recorded cost in `perf/baseline.json` is a whole number of microseconds divided by
/// its unit count - and `pane.encoder`'s entire iteration was a single tick, making its
/// recorded 66.67 ns/pane the clock's resolution over fifteen panes rather than a cost. A
/// quantity that steps by 100% cannot be judged at a 2.00x tolerance. This harness never
/// crosses a process, so it can have the finer clock.
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
        let start = Instant::now();
        body();
        #[allow(clippy::cast_precision_loss)]
        samples.push(start.elapsed().as_nanos() as f64);
    }

    samples.sort_by(f64::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let units = units_per_iteration as f64;
    Cost {
        name: name.to_string(),
        unit: unit.to_string(),
        best: samples[0] / units,
        median: samples[samples.len() / 2] / units,
        iterations,
    }
}

/// The smallest interval this harness's clock can tell apart, in nanoseconds.
///
/// Measured rather than assumed, because the answer is per-platform and getting it wrong is
/// invisible in the output: every cost comes back looking like a number. It goes through the
/// same clock `measure` uses, so a change of clock changes this too - which is the point,
/// since the defect it guards against is exactly that swap having been made once already.
///
/// An upper bound: two readings and the work between them, so the true resolution is finer.
/// That is the right direction for the one question asked of it - can this clock describe a
/// benchmark at all - because a bound that is too pessimistic never passes a bad clock.
pub fn clock_resolution_nanos() -> f64 {
    let mut smallest = f64::INFINITY;
    for _ in 0..200 {
        let start = Instant::now();
        let mut gap = 0.0;
        while gap <= 0.0 {
            gap = start.elapsed().as_secs_f64() * 1e9;
        }
        smallest = smallest.min(gap);
    }
    smallest
}
