//! Whether a pane's program has stopped reading its input.
//!
//! A program that stops reading leaves what is sent to it in its terminal, where the daemon can
//! count it ([`crate::pty::queued`]). Only typed input counts toward `stuck`. The pane then looks
//! like any other quiet one - it paints nothing because it is not reading - and a window blamed its
//! bridge, `muster window` called it idle, and a message rang it for an answer that never came
//! (GitHub issue #1).
//!
//! Bytes waiting are not enough on their own. A program still starting - an agent loading its
//! plugins, a binary held in macOS's first-exec scan - has not read anything yet, and input sent
//! as it was launched waits for it legitimately. So a program is called stopped only once it has
//! been seen to read: a count that went down, or nothing left of input written since the last
//! look - input that ended a line, when the terminal passes input on a line at a time and so
//! counts half a line as nothing. That evidence belongs to the program
//! in the foreground, and goes when another takes its place, so a shell that read lends nothing
//! to the agent it starts.
//!
//! The decision takes the probe and clock as arguments, so its rules can be tested without a PTY.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pty::Queued;

/// How long input may wait unread, with nothing read meanwhile, before a program that has been
/// reading is called stopped. Longer than any pause a program that is reading takes - a
/// terminal program reads a keystroke in milliseconds - and short enough to have been noticed
/// before a window's own watch on a silent pane says anything (10 s).
pub(crate) const STUCK_AFTER: Duration = Duration::from_secs(5);

/// A debug build's [`STUCK_AFTER`] in milliseconds, when set, so a test need not wait for it.
const STUCK_AFTER_SAID: &str = "MUSTER_DAEMON_STUCK_MS";

/// How often a pane is looked at while it has anything to say: often enough that a stop is
/// noticed within a second of [`STUCK_AFTER`], and a pane that reads again is cleared as soon.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// More bytes than FIONREAD can see on either platform (4095 on Linux, 1024 on macOS).
const HISTORY_BYTES: u32 = 8192;

/// The kinds of bytes the writer has passed to the PTY, in write order. It stores lengths only.
#[derive(Debug, Default)]
pub(crate) struct Writes {
    recent: VecDeque<Run>,
    recent_bytes: u32,
    inherited: bool,
    pub(crate) typed_at: Option<Instant>,
    pub(crate) lines: u64,
    ignored_bytes: u64,
    ignored_at_line_end: u64,
    ignored_through_read_line: u64,
}

#[derive(Debug)]
struct Run {
    bytes: u32,
    typed: bool,
}

impl Writes {
    pub(crate) fn new(inherited: bool) -> Self {
        Self { inherited, ..Self::default() }
    }

    pub(crate) fn flushed(&mut self) {
        *self = Self { lines: self.lines, typed_at: self.typed_at, ..Self::default() };
    }

    pub(crate) fn written(&mut self, written: &[u8], typed: bool) {
        if written.is_empty() {
            return;
        }
        let len = written.len() as u64;
        if typed {
            self.typed_at = Some(Instant::now());
        }
        if let Some(end) = written.iter().rposition(|&byte| byte == b'\r' || byte == b'\n') {
            self.lines += u64::from(typed);
            self.ignored_at_line_end = self.ignored_bytes + if typed { 0 } else { end as u64 + 1 };
        }
        if !typed {
            self.ignored_bytes += len;
        }

        let bytes = u32::try_from(len).unwrap_or(HISTORY_BYTES).min(HISTORY_BYTES);
        self.recent_bytes += bytes;
        if let Some(last) = self.recent.back_mut().filter(|run| run.typed == typed) {
            last.bytes += bytes;
        } else {
            self.recent.push_back(Run { bytes, typed });
        }
        while self.recent_bytes > HISTORY_BYTES {
            let first = self.recent.front_mut().expect("some bytes are recorded");
            let removed = first.bytes.min(self.recent_bytes - HISTORY_BYTES);
            first.bytes -= removed;
            self.recent_bytes -= removed;
            if first.bytes == 0 {
                self.recent.pop_front();
            }
        }
    }

    pub(crate) fn queued(&mut self, mut queued: Queued) -> Queued {
        if queued.bytes == 0 {
            self.inherited = false;
            self.ignored_through_read_line =
                if queued.canonical { self.ignored_at_line_end } else { self.ignored_bytes };
        } else if queued.canonical {
            // The line discipline can edit the bytes it accepts. Subtract every ignored byte on
            // a line that might still be queued, leaving a lower bound on typed bytes.
            let ignored = self.ignored_at_line_end.saturating_sub(self.ignored_through_read_line);
            queued.bytes = queued.bytes.saturating_sub(u32::try_from(ignored).unwrap_or(u32::MAX));
        } else {
            // Reads take bytes from the head. FIONREAD may omit bytes still moving through the
            // kernel's buffers, but the last bytes written up to that count are still unread.
            let mut remaining = queued.bytes;
            let mut typed = 0;
            for run in self.recent.iter().rev() {
                let bytes = run.bytes.min(remaining);
                if run.typed {
                    typed += bytes;
                }
                remaining -= bytes;
                if remaining == 0 {
                    break;
                }
            }
            // A handoff can keep a stopped pane before the new writer has seen its first byte.
            queued.bytes = typed + if self.inherited { remaining } else { 0 };
        }
        queued
    }
}

pub(crate) fn stuck_after() -> Duration {
    if cfg!(debug_assertions)
        && let Ok(said) = std::env::var(STUCK_AFTER_SAID)
        && let Ok(millis) = said.trim().parse::<u64>()
    {
        return Duration::from_millis(millis);
    }
    STUCK_AFTER
}

/// One look at a pane: who holds its terminal, and what it has not read. `queued` is None when
/// the terminal could not be read, which says nothing either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Look {
    pub(crate) group: Option<i32>,
    pub(crate) queued: Option<Queued>,
    /// How many writes of typed input to the pane have ended a line, ever.
    pub(crate) lines: u64,
}

/// What changed about a pane's program, when anything did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// It has stopped reading: `bytes` wait, unread since `since`.
    Stopped { bytes: u32, since: Instant },
    /// It is no longer called stopped, having been for `stopped_for`.
    Cleared { stopped_for: Duration, why: Cleared },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleared {
    /// It read.
    Read,
    /// Another program holds the terminal now.
    Replaced,
    /// The terminal could not be read, so nothing is known.
    Unknown,
}

impl Cleared {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Cleared::Read => "read",
            Cleared::Replaced => "replaced",
            Cleared::Unknown => "unknown",
        }
    }
}

/// One pane's program, as far as reading its input goes.
#[derive(Debug)]
pub(crate) struct Unread {
    stuck_after: Duration,
    /// The foreground group the rest is about.
    group: Option<i32>,
    /// That group has been seen to read.
    has_read: bool,
    /// What the last look found waiting, while it found anything.
    waiting: Option<u32>,
    /// Since when input has waited with nothing read.
    since: Option<Instant>,
    /// When it was called stopped, while it is.
    stopped: Option<Instant>,
    /// When the pane was last looked at.
    looked: Option<Instant>,
    /// When input had last been written as of that look.
    written: Option<Instant>,
    /// How many writes had ended a line as of that look.
    lines: u64,
}

impl Unread {
    pub(crate) fn new(stuck_after: Duration) -> Unread {
        Unread {
            stuck_after,
            group: None,
            has_read: false,
            waiting: None,
            since: None,
            stopped: None,
            looked: None,
            written: None,
            lines: 0,
        }
    }

    /// A program a daemon before this one had already called stopped, holding the terminal as
    /// `group`: stopped until it reads, as it was there.
    pub(crate) fn stopped(stuck_after: Duration, group: Option<i32>, now: Instant) -> Unread {
        Unread { group, has_read: true, stopped: Some(now), ..Unread::new(stuck_after) }
    }

    /// Whether the pane is worth looking at now, given when input was last written to it. A
    /// pane nothing was written to lately, and that had nothing waiting, has nothing to say, so
    /// a quiet pane costs nothing.
    pub(crate) fn due(&self, input_at: Option<Instant>, now: Instant) -> bool {
        if self.looked.is_some_and(|looked| now < looked + LOOK_EVERY) {
            return false;
        }
        let lately =
            input_at.is_some_and(|at| now.saturating_duration_since(at) < self.stuck_after);
        self.waiting.is_some() || self.stopped.is_some() || lately || input_at != self.written
    }

    /// Takes a look, `input_at` being when input was last written to the pane.
    pub(crate) fn looked(
        &mut self,
        look: Look,
        input_at: Option<Instant>,
        now: Instant,
    ) -> Option<Change> {
        let written = input_at.is_some() && input_at != self.written;
        let line = look.lines > self.lines;
        self.written = input_at;
        self.lines = look.lines;
        self.looked = Some(now);
        if look.group != self.group {
            self.group = look.group;
            self.has_read = false;
            self.waiting = None;
            self.since = None;
            // What this look found is the new program's, and its own start of evidence.
            let replaced = self.clear(now, Cleared::Replaced);
            let judged = self.judge(look, written && (line || !canonical(look)), now);
            return replaced.or(judged);
        }
        self.judge(look, written && (line || !canonical(look)), now)
    }

    /// Judges a look by the program already in the foreground, `taken` saying whether input that
    /// would count if unread was written since the last look.
    fn judge(&mut self, look: Look, taken: bool, now: Instant) -> Option<Change> {
        let Some(queued) = look.queued else {
            self.waiting = None;
            self.since = None;
            return self.clear(now, Cleared::Unknown);
        };
        let went_down = self.waiting.is_some_and(|before| queued.bytes < before);
        if went_down || (queued.bytes == 0 && taken) {
            self.has_read = true;
        }
        if queued.bytes == 0 || went_down {
            self.waiting = Some(queued.bytes).filter(|&bytes| bytes > 0);
            self.since = self.waiting.map(|_| now);
            return self.clear(now, Cleared::Read);
        }
        self.waiting = Some(queued.bytes);
        let since = *self.since.get_or_insert(now);
        if self.stopped.is_none()
            && self.has_read
            && now.saturating_duration_since(since) >= self.stuck_after
        {
            self.stopped = Some(now);
            return Some(Change::Stopped { bytes: queued.bytes, since });
        }
        None
    }

    fn clear(&mut self, now: Instant, why: Cleared) -> Option<Change> {
        let stopped = self.stopped.take()?;
        Some(Change::Cleared { stopped_for: now.saturating_duration_since(stopped), why })
    }
}

/// Whether the look found the terminal passing input on a line at a time, where half a line
/// counts as nothing waiting. Unknown counts as yes, the cautious answer.
fn canonical(look: Look) -> bool {
    look.queued.is_none_or(|queued| queued.canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: Duration = Duration::from_secs(5);
    const SECOND: Duration = Duration::from_secs(1);

    fn raw(bytes: u32) -> Look {
        Look { group: Some(10), queued: Some(Queued { bytes, canonical: false }), lines: 0 }
    }

    /// Line by line, `lines` writes having ended a line.
    fn lines(bytes: u32, lines: u64) -> Look {
        Look { group: Some(10), queued: Some(Queued { bytes, canonical: true }), lines }
    }

    #[test]
    fn queue_progress_removes_the_oldest_bytes_first() {
        let mut writes = Writes::default();
        writes.written(b"abc", true);
        writes.written(b"pointer", false);
        writes.written(b"de", true);
        writes.written(b"focus", false);

        let typed = |writes: &mut Writes, bytes| writes.queued(raw(bytes).queued.unwrap()).bytes;
        assert_eq!(typed(&mut writes, 17), 5);
        assert_eq!(typed(&mut writes, 14), 2);
        assert_eq!(typed(&mut writes, 7), 2);
        assert_eq!(typed(&mut writes, 6), 1);
        assert_eq!(typed(&mut writes, 5), 0);
    }

    #[test]
    fn a_canonical_queue_counts_only_typed_bytes_on_completed_lines() {
        let mut writes = Writes::default();
        writes.written(b"pointer", false);
        writes.written(b"abc\r", true);
        writes.written(b"another pointer", false);

        let typed =
            |writes: &mut Writes, bytes| writes.queued(lines(bytes, 0).queued.unwrap()).bytes;
        assert_eq!(typed(&mut writes, 11), 4);
        assert_eq!(typed(&mut writes, 0), 0);
        writes.written(b"d\r", true);
        assert_eq!(typed(&mut writes, 17), 2);
    }

    /// Looks once a second from `start`, each look with input last written at `written`,
    /// and says what the last one changed.
    fn looks(
        unread: &mut Unread,
        start: Instant,
        seconds: u64,
        look: Look,
        written: Option<Instant>,
    ) -> Vec<Change> {
        (0..seconds)
            .filter_map(|second| {
                unread.looked(look, written, start + SECOND * u32::try_from(second).unwrap())
            })
            .collect()
    }

    /// A program that had been reading - a keystroke taken as it came - and then leaves what is
    /// sent unread is stopped once it has waited [`STUCK_AFTER`], and reads as reading again as
    /// soon as the count goes down.
    #[test]
    fn a_program_that_was_reading_and_stops_is_stopped_until_it_reads() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        assert_eq!(unread.looked(raw(0), Some(start), start), None, "it took the keystroke");
        let typed = start + SECOND;
        let changes = looks(&mut unread, typed, 7, raw(2817), Some(typed));
        assert_eq!(changes, vec![Change::Stopped { bytes: 2817, since: typed }]);
        let read = unread.looked(raw(100), Some(typed), typed + SECOND * 8);
        assert!(matches!(read, Some(Change::Cleared { why: Cleared::Read, .. })), "{read:?}");
    }

    /// A program that has never been seen reading - one still starting, with input sent as it
    /// was launched - is never stopped, however long the input waits.
    #[test]
    fn a_program_never_seen_reading_is_never_stopped() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        assert!(looks(&mut unread, start, 60, raw(40), Some(start)).is_empty());
        assert_eq!(unread.looked(raw(0), Some(start), start + SECOND * 60), None);
    }

    /// Line by line, half a line counts as nothing, so nothing left after typing is not evidence
    /// of reading: whole lines then waiting from a program still starting do not stop it.
    #[test]
    fn half_a_line_counted_as_nothing_is_not_reading() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        unread.looked(lines(0, 0), Some(start), start);
        let entered = start + SECOND;
        assert!(looks(&mut unread, entered, 10, lines(6, 1), Some(entered)).is_empty());
    }

    /// Line by line, nothing left of a line written since the last look is that line read: a
    /// program reading a line at a time that then leaves one unread is stopped.
    #[test]
    fn a_line_taken_line_by_line_is_reading() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        unread.looked(lines(0, 1), Some(start), start);
        let entered = start + SECOND;
        let changes = looks(&mut unread, entered, 7, lines(6, 2), Some(entered));
        assert_eq!(changes, vec![Change::Stopped { bytes: 6, since: entered }]);
    }

    /// The evidence belongs to the program in the foreground: a shell that read lends nothing to
    /// the program it starts, and a program called stopped is cleared when another takes over.
    #[test]
    fn evidence_goes_with_the_program_that_gave_it() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        unread.looked(raw(0), Some(start), start);
        let started = Look { group: Some(11), ..raw(30) };
        let changes = looks(&mut unread, start + SECOND, 10, started, Some(start + SECOND));
        assert!(changes.is_empty(), "{changes:?}");

        let mut stopped = Unread::new(AFTER);
        stopped.looked(raw(0), Some(start), start);
        looks(&mut stopped, start + SECOND, 7, raw(9), Some(start + SECOND));
        let replaced = stopped.looked(Look { group: Some(12), ..raw(9) }, None, start + SECOND * 9);
        assert!(
            matches!(replaced, Some(Change::Cleared { why: Cleared::Replaced, .. })),
            "{replaced:?}"
        );
    }

    /// A count that goes down is a read, and starts the wait over: a program draining slowly is
    /// reading, however long its input waits in all.
    #[test]
    fn a_program_draining_slowly_is_reading() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        let changes: Vec<Change> = (0..20u32)
            .filter_map(|second| {
                unread.looked(raw(1000 - second * 10), Some(start), start + SECOND * second)
            })
            .collect();
        assert!(changes.is_empty(), "{changes:?}");
    }

    /// A terminal that cannot be read says nothing, so a program called stopped is no longer.
    #[test]
    fn a_terminal_that_cannot_be_read_says_nothing() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        unread.looked(raw(0), Some(start), start);
        looks(&mut unread, start + SECOND, 7, raw(9), Some(start + SECOND));
        let unknown = unread.looked(Look { queued: None, ..raw(0) }, None, start + SECOND * 9);
        assert!(
            matches!(unknown, Some(Change::Cleared { why: Cleared::Unknown, .. })),
            "{unknown:?}"
        );
    }

    /// Looked at only while there is something to see, and never more than once a second.
    #[test]
    fn a_quiet_pane_is_not_looked_at() {
        let start = Instant::now();
        let mut unread = Unread::new(AFTER);
        assert!(!unread.due(None, start), "nothing was ever written");
        assert!(unread.due(Some(start), start), "input was just written");
        unread.looked(raw(0), Some(start), start);
        assert!(!unread.due(Some(start), start + SECOND / 2), "looked at a moment ago");
        assert!(unread.due(Some(start), start + SECOND), "written lately");
        assert!(!unread.due(Some(start), start + AFTER + SECOND), "written long ago, and read");
        unread.looked(raw(0), Some(start), start + SECOND);
        let later = start + SECOND * 30;
        assert!(unread.due(Some(later), later + SECOND * 10), "written since the last look");
    }
}
