//! What a pane's bridge tells the window about itself, on the socket the window binds for it.
//!
//! One way only: the bridge dials, reports, and the window listens. Input goes to the daemon
//! and nothing needs saying to a bridge, so nothing travels the other way.
//!
//! **The socket closing is how the window learns a bridge died**, whatever killed it: libghostty's
//! own close callback never arrived in two field runs (kan a_2IRcMjFs0), and the daemon cannot
//! tell one window's bridge from another's, or say anything at all when the connection to it is
//! down. The reports say the rest, which only the bridge knows:
//!
//! - **that it attached**, so a pane still waiting for a bridge can be told from one that has one;
//! - **that it is painting**, since output runs from its stdout into a surface and never passes
//!   the window, so a pane that stopped answering looks exactly like one nobody has touched;
//! - **why it is exiting**, which decides whether the window starts another.
//!
//! A line per report, words separated by spaces, with the reason last so it may hold spaces.

use crate::respawn::{Ended, Ending};

/// How often a bridge says it painted, while it is painting, in nanoseconds.
pub const PAINTED_INTERVAL_NS: u64 = 250_000_000;

/// One thing a bridge says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The daemon accepted the bridge's stream for its pane.
    Attached,
    /// Output reached the surface since the last report: how many writes, and how many bytes.
    Painted { writes: u64, bytes: u64 },
    /// The bridge is about to exit, and why.
    Exiting(Ended),
}

impl Report {
    /// The line this report travels as, newline included.
    pub fn line(&self) -> String {
        match self {
            Report::Attached => "attached\n".to_string(),
            Report::Painted { writes, bytes } => format!("painted {writes} {bytes}\n"),
            Report::Exiting(ended) => format!(
                "exiting {} {} {}\n",
                ended.ending.as_str(),
                u8::from(ended.rendered),
                // One line, whatever the daemon's sentence held.
                ended.reason.as_deref().unwrap_or_default().replace(['\n', '\r'], " ")
            ),
        }
    }

    /// Reads one line, without its newline. Nothing for a line that is not a report, which a
    /// listener logs and skips: a bridge from another build may say something this one does
    /// not know.
    pub fn parse(line: &str) -> Option<Report> {
        let mut words = line.splitn(4, ' ');
        match words.next()? {
            "attached" => Some(Report::Attached),
            "painted" => Some(Report::Painted {
                writes: words.next()?.parse().ok()?,
                bytes: words.next()?.parse().ok()?,
            }),
            "exiting" => {
                let ending = Ending::parse(words.next()?)?;
                let rendered = words.next()? == "1";
                let reason = words.next().map(str::trim).filter(|reason| !reason.is_empty());
                Some(Report::Exiting(Ended {
                    ending,
                    reason: reason.map(str::to_string),
                    rendered,
                }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(report: &Report) -> Option<Report> {
        let line = report.line();
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line:?}");
        Report::parse(line.trim_end_matches('\n'))
    }

    #[test]
    fn every_report_reads_back_as_itself() {
        let reports = [
            Report::Attached,
            Report::Painted { writes: 12, bytes: 4096 },
            Report::Exiting(Ended {
                ending: Ending::TakenOver,
                reason: Some("another window attached to p1w3r07bsd".to_string()),
                rendered: true,
            }),
            Report::Exiting(Ended { ending: Ending::Lost, reason: None, rendered: false }),
        ];
        for report in reports {
            assert_eq!(round_trip(&report), Some(report.clone()));
        }
    }

    /// The daemon's sentence can hold a newline, and a report is one line.
    #[test]
    fn a_reason_across_lines_stays_one_report() {
        let report = Report::Exiting(Ended {
            ending: Ending::Gone,
            reason: Some("the pane closed\nwhile attached".to_string()),
            rendered: false,
        });
        let Some(Report::Exiting(read)) = round_trip(&report) else { panic!("not a report") };
        assert_eq!(read.reason.as_deref(), Some("the pane closed while attached"));
    }

    #[test]
    fn a_line_that_is_not_a_report_is_none() {
        for line in ["", "hello", "painted many", "exiting sideways 1", "painted 1"] {
            assert_eq!(Report::parse(line), None, "{line:?}");
        }
    }
}
