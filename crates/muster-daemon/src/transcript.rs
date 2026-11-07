//! Whether a compaction Claude Code's hooks announced was the session's own.
//!
//! A sub-agent's compaction fires `PostCompact` with its parent's session id and transcript path
//! and no `agent_id`, so the hook's input cannot tell the two apart. The transcripts can: each
//! agent's compaction leaves its `compact_boundary` record in that agent's own file
//! (`docs/observations/claude-code-2.1.319.md`). The record lands a moment after the hook starts,
//! so a report has to wait for it, which is why its hook runs asynchronously. A sub-agent's hook
//! waiting here can find a boundary its parent writes meanwhile, so what comes back is the
//! boundary's id, which lets the daemon take one compaction once however many hooks found it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use muster_core::diagnostics::clock::format_iso8601;

/// How long a report waits for the record. Measured in tens of milliseconds, on a harness that
/// may have a large transcript to write.
const PATIENCE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

/// How far back from the hook's start a record counts: the two clocks are one machine's, and the
/// record's time is taken before the hook starts on a slower run.
const SLACK_MS: u64 = 1000;

/// How much of the transcript's end is read. A record is under a kilobyte, and what follows it
/// by the time this looks is the compaction's summary.
const TAIL_BYTES: u64 = 1 << 20;

/// The id of the compaction the Claude Code transcript at `path` records from around
/// `started_ms`, waiting for it.
pub(crate) fn recorded_compaction(path: &Path, started_ms: u64) -> Option<String> {
    recorded_within(path, started_ms, PATIENCE)
}

fn recorded_within(path: &Path, started_ms: u64, patience: Duration) -> Option<String> {
    let millis = i64::try_from(started_ms.saturating_sub(SLACK_MS)).unwrap_or(i64::MAX);
    let since = format_iso8601(millis);
    let deadline = Instant::now() + patience;
    loop {
        if let Some(boundary) = tail(path).and_then(|text| boundary_since(&text, &since)) {
            return Some(boundary);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

fn tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The id of the first boundary in `text` timestamped `since` or later. Both times are
/// `format_iso8601`'s shape, so they compare as text. A line cut off by the tail or still being
/// written reads as no record.
fn boundary_since(text: &str, since: &str) -> Option<String> {
    text.lines().filter(|line| line.contains("compact_boundary")).find_map(|line| {
        let record = serde_json::from_str::<serde_json::Value>(line).ok()?;
        let recent = record["timestamp"].as_str().is_some_and(|at| at >= since);
        (record["subtype"] == "compact_boundary" && recent)
            .then(|| record["uuid"].as_str().map(str::to_string))?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Selected from the transcript of the session in `context-hooks.jsonl`.
    const RECORDED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/claude-code-2.1.319/compact-boundary.jsonl"
    ));
    /// `RECORDED`'s boundary, in milliseconds.
    const BOUNDARY_MS: u64 = 1_794_016_448_338;
    const BOUNDARY_ID: &str = "dff6728a-c3c5-4cdf-a242-3375f27125c5";

    fn from_boundary(offset_ms: i64) -> String {
        format_iso8601(i64::try_from(BOUNDARY_MS).unwrap() + offset_ms)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir()
            .join(format!("muster-transcript-{name}-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_recorded_boundary_counts_from_around_the_hooks_start() {
        let found = boundary_since(RECORDED, &from_boundary(-25));
        assert_eq!(found.as_deref(), Some(BOUNDARY_ID));
    }

    #[test]
    fn a_boundary_from_before_the_hook_is_another_compaction() {
        assert_eq!(boundary_since(RECORDED, &from_boundary(1)), None);
    }

    #[test]
    fn the_word_alone_is_not_a_boundary() {
        let quoted =
            r#"{"type":"user","text":"compact_boundary","timestamp":"2099-01-01T00:00:00.000Z"}"#;
        assert_eq!(boundary_since(quoted, "2000-01-01T00:00:00.000Z"), None);
        assert_eq!(boundary_since(r#"{"subtype":"compact_boundary","timest"#, "2000"), None);
    }

    #[test]
    fn a_boundary_written_after_the_report_starts_is_waited_for() {
        let path = scratch("late");
        std::fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                file.write_all(RECORDED.as_bytes()).unwrap();
            })
        };
        let found = recorded_within(&path, BOUNDARY_MS, Duration::from_secs(10));
        assert_eq!(found.as_deref(), Some(BOUNDARY_ID));
        writer.join().unwrap();
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_transcript_that_stays_silent_is_a_sub_agents_compaction() {
        let path = scratch("silent");
        std::fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
        assert_eq!(recorded_within(&path, BOUNDARY_MS, Duration::from_millis(150)), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn no_transcript_is_no_record() {
        assert_eq!(recorded_within(&scratch("missing"), BOUNDARY_MS, Duration::ZERO), None);
    }
}
