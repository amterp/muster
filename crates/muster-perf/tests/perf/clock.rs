//! The harness is only as good as the clock under it, and a clock too coarse for the work
//! being timed does not fail - it reports a number.
//!
//! This is not hypothetical. Every cost in the first `perf/baseline.json` is a whole number of
//! microseconds divided by its unit count, because the harness timed through the core's
//! `CLOCK_MONOTONIC` and on Darwin that resolves exactly 1 µs. `pane.encoder`'s entire
//! iteration was one tick, so the 66.67 ns/pane it held a budget with for a week was the
//! clock's resolution over fifteen panes.

use muster_perf::{RESOLVABLE_NANOS, clock_resolution_nanos};

#[test]
fn the_harness_clock_resolves_finer_than_a_benchmark_needs() {
    let resolution = clock_resolution_nanos();

    // Lenient by design: it fails only where the harness genuinely cannot describe the work it
    // is timing, which is the machine on which every number it prints is fiction. The floor is
    // the same one `compare` refuses to judge below, so a clock that passes here can resolve
    // anything the harness will agree to gate.
    assert!(
        resolution < RESOLVABLE_NANOS,
        "the harness clock resolves {resolution} ns, which is coarser than the {RESOLVABLE_NANOS} \
         ns floor below which a measurement is mostly timing overhead"
    );
}
