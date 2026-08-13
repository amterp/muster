//! The perf harness's decidable half: what a measurement is, how a run is judged against a
//! baseline, and how the verdict reads.
//!
//! Separated from the binary that runs the timing loops, so the part that decides whether
//! to fail a build is itself tested.

mod benchmark;
mod cost;
mod report;

pub use benchmark::measure;
pub use cost::{Baseline, Comparison, Cost, Regression, compare};
pub use report::{pending, table, verdict};
