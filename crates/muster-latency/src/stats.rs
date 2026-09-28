//! Summaries of timed samples, the table they print as, and the verdicts MIP-3's targets give.

use std::fmt::Write;

/// Timings of one path, in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) name: String,
    pub(crate) samples: usize,
    pub(crate) min: f64,
    pub(crate) median: f64,
    pub(crate) p95: f64,
    pub(crate) max: f64,
}

/// Nearest rank, so every percentile reported is a sample that was actually measured.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let rank = (sorted.len() as f64 * fraction) as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[middle]
    }
}

/// `None` for no samples, which a path that never answered leaves behind.
pub(crate) fn summarize(name: &str, samples: &[f64]) -> Option<Row> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Row {
        name: name.to_string(),
        samples: sorted.len(),
        min: sorted[0],
        median: median(&sorted),
        p95: percentile(&sorted, 0.95),
        max: sorted[sorted.len() - 1],
    })
}

pub(crate) fn render(rows: &[Row]) -> String {
    let mut table =
        format!("{:46}  {:>8}  {:>8}  {:>8}  {:>8}\n", "path", "min", "median", "p95", "max");
    for row in rows {
        let _ = writeln!(
            table,
            "{:46}  {:8.2}  {:8.2}  {:8.2}  {:8.2}",
            row.name, row.min, row.median, row.p95, row.max
        );
    }
    table
}

/// One of MIP-3 section 13's targets, and whether a run met it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Verdict {
    pub(crate) target: String,
    pub(crate) measured: String,
    pub(crate) met: bool,
}

impl Verdict {
    pub(crate) fn line(&self) -> String {
        format!(
            "{}  {}: {}",
            if self.met { "met   " } else { "MISSED" },
            self.target,
            self.measured
        )
    }
}

/// Input-to-glyph against the floor: the median within 0.5 ms of it, the p95 within 1 ms. The
/// p95 target is also the "no second mode" one: a path that sometimes waits for something shows
/// it there first.
pub(crate) fn against_floor(label: &str, path: &Row, floor: &Row) -> Vec<Verdict> {
    let median = path.median - floor.median;
    let p95 = path.p95 - floor.p95;
    vec![
        Verdict {
            target: format!("{label}: median within 0.5 ms of the bare PTY"),
            measured: format!("{median:+.2} ms"),
            met: median <= 0.5,
        },
        Verdict {
            target: format!("{label}: p95 within 1 ms of the bare PTY"),
            measured: format!("{p95:+.2} ms"),
            met: p95 <= 1.0,
        },
    ]
}

/// A keystroke's echo beside a flood, against the same echo with no flood.
pub(crate) fn beside_flood(alone: &Row, flooded: &Row) -> Vec<Verdict> {
    let median = flooded.median - alone.median;
    let p95 = flooded.p95 - alone.p95;
    vec![Verdict {
        target: "echo beside a flood within 1 ms of the same echo alone".to_string(),
        measured: format!("median {median:+.2} ms, p95 {p95:+.2} ms"),
        met: median <= 1.0 && p95 <= 1.0,
    }]
}

/// What a nice-10 build beside the daemon may cost the requests an agent makes: reads within
/// a budget of their own, and an echo within 1 ms of the same echo idle. A daemon whose threads
/// run below the build's priority misses all three by tens of milliseconds or more.
pub(crate) fn beside_a_build(tail: &Row, whole: &Row, echo_idle: &Row, echo: &Row) -> Vec<Verdict> {
    let echo_p95 = echo.p95 - echo_idle.p95;
    vec![
        Verdict {
            target: format!(
                "a read of the last rows beside a build: p95 within {TAIL_BUDGET_MS} ms"
            ),
            measured: format!("{:.2} ms", tail.p95),
            met: tail.p95 <= TAIL_BUDGET_MS,
        },
        Verdict {
            target: format!(
                "a read of the whole history beside a build: p95 within {WHOLE_BUDGET_MS} ms"
            ),
            measured: format!("{:.2} ms", whole.p95),
            met: whole.p95 <= WHOLE_BUDGET_MS,
        },
        Verdict {
            target: "echo beside a build: p95 within 1 ms of the same echo idle".to_string(),
            measured: format!("{echo_p95:+.2} ms"),
            met: echo_p95 <= 1.0,
        },
    ]
}

const TAIL_BUDGET_MS: f64 = 10.0;
const WHOLE_BUDGET_MS: f64 = 50.0;

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact halves and whole numbers
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_nearest_rank_like_the_python_half() {
        let samples: Vec<f64> = (1..=20).map(f64::from).collect();
        let row = summarize("x", &samples).unwrap();
        assert_eq!((row.min, row.median, row.p95, row.max), (1.0, 10.5, 20.0, 20.0));
        assert_eq!(summarize("x", &[3.0, 1.0, 2.0]).unwrap().median, 2.0);
        assert_eq!(summarize("x", &[]), None);
    }

    #[test]
    fn a_path_is_judged_against_the_floor() {
        let floor = summarize("floor", &[0.1, 0.1, 0.2]).unwrap();
        let near = summarize("near", &[0.3, 0.4, 0.5]).unwrap();
        let far = summarize("far", &[1.0, 1.0, 3.0]).unwrap();
        assert!(against_floor("near", &near, &floor).iter().all(|verdict| verdict.met));
        let judged = against_floor("far", &far, &floor);
        assert!(!judged[0].met, "a median 0.9 ms over");
        assert!(!judged[1].met, "a p95 2.8 ms over");
    }

    #[test]
    fn a_flood_is_judged_against_the_same_echo_alone() {
        let alone = summarize("alone", &[0.2, 0.2, 0.3]).unwrap();
        let fine = summarize("fine", &[0.3, 0.4, 0.9]).unwrap();
        let held = summarize("held", &[0.3, 0.4, 100.0]).unwrap();
        assert!(beside_flood(&alone, &fine)[0].met);
        assert!(!beside_flood(&alone, &held)[0].met);
    }

    #[test]
    fn a_build_beside_the_daemon_is_judged_by_its_own_budgets() {
        let echo = summarize("echo", &[0.2, 0.3, 0.9]).unwrap();
        let (tail, whole) =
            (summarize("tail", &[1.0]).unwrap(), summarize("whole", &[9.0]).unwrap());
        assert!(beside_a_build(&tail, &whole, &echo, &echo).iter().all(|verdict| verdict.met));
        let slow = summarize("slow", &[0.2, 5.0, 60.0]).unwrap();
        let judged = beside_a_build(&slow, &slow, &echo, &slow);
        assert_eq!(
            judged.iter().map(|verdict| verdict.met).collect::<Vec<_>>(),
            [false, false, false]
        );
    }
}
