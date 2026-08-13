//! A log file several processes append to at once.
//!
//! The app and every bridge it spawns share one file, because the questions worth asking
//! span them: a keystroke leaves the app and arrives at a bridge, and two files with two
//! clocks make that a correlation exercise instead of a read.
//!
//! Concurrent appends are safe because each record is one `write` to an `O_APPEND`
//! descriptor, which the kernel places at the end atomically, and records are capped below
//! the size where a single write could be split. Fixed key order and sorted fields keep
//! the output diffable.

use std::ffi::CString;
use std::fmt::Write as _;

use super::clock::format_iso8601;
use super::log::{LogRecord, LogSink};

/// Records longer than this are truncated rather than risking a torn line.
///
/// A split record would corrupt the line either side of it, so losing the tail of one
/// oversized record is the cheaper failure.
const MAXIMUM_RECORD_BYTES: usize = 4096;

#[derive(Debug)]
pub struct JsonLinesSink {
    fd: i32,
}

impl JsonLinesSink {
    /// Opens the file, creating it if needed, or returns none if it cannot be written.
    ///
    /// Logging never takes the process down with it: a diagnostic that can fail the thing
    /// it diagnoses is worse than no diagnostic.
    pub fn open(path: &str) -> Option<JsonLinesSink> {
        let c_path = CString::new(path).ok()?;
        // SAFETY: the path is a NUL-terminated string we own for the duration of the call,
        // and open touches nothing else.
        let fd = unsafe {
            libc::open(c_path.as_ptr(), libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT, 0o644)
        };
        if fd < 0 { None } else { Some(JsonLinesSink { fd }) }
    }
}

impl Drop for JsonLinesSink {
    fn drop(&mut self) {
        // SAFETY: the descriptor was opened by this type and is closed exactly once.
        unsafe { libc::close(self.fd) };
    }
}

impl LogSink for JsonLinesSink {
    fn write(&self, record: &LogRecord) {
        let mut line = encode(record);
        if line.len() > MAXIMUM_RECORD_BYTES {
            line = truncated(&line);
        }
        line.push('\n');

        let bytes = line.as_bytes();
        let mut written = 0;
        while written < bytes.len() {
            // SAFETY: the slice outlives the call and the descriptor is open for as long
            // as this sink is.
            let n = unsafe {
                libc::write(
                    self.fd,
                    bytes[written..].as_ptr().cast::<libc::c_void>(),
                    bytes.len() - written,
                )
            };
            // Nowhere to report a failed log write to, so it is dropped. Retrying forever
            // would hang whatever thread was trying to say something.
            match usize::try_from(n) {
                Ok(0) | Err(_) => return,
                Ok(step) => written += step,
            }
        }
    }
}

/// Renders one record, with the identifying keys first and the payload after.
///
/// Hand-built rather than through a serializer so the key order is the one a human reads
/// well: when, how precisely, how bad, who, what - then the details, sorted so two runs of
/// the same code produce the same bytes.
///
/// Both clocks, on every line. `time` is what a person reads; `mono_ns` is what the perf
/// harness subtracts, and it has to be on the ordinary records rather than on a separate
/// timing channel, because the hops worth measuring are the ones already being logged.
pub fn encode(record: &LogRecord) -> String {
    let mut out = String::with_capacity(160);
    out.push_str("{\"time\":\"");
    out.push_str(&format_iso8601(record.time_ms));
    out.push('"');
    let _ = write!(out, ",\"mono_ns\":{}", record.mono);
    let _ = write!(out, ",\"level\":\"{}\"", record.level.as_str());
    let _ = write!(out, ",\"process\":\"{}\"", record.process);
    let _ = write!(out, ",\"pid\":{}", record.pid);
    out.push_str(",\"event\":");
    quote_into(&mut out, &record.event);
    for (key, value) in &record.fields {
        out.push(',');
        quote_into(&mut out, key);
        out.push(':');
        quote_into(&mut out, value);
    }
    out.push('}');
    out
}

/// Cuts an oversized record down and closes the object.
///
/// A last resort, and it shows: the cut can land inside an escape sequence, so the result
/// is a line that says it was truncated rather than a line that is guaranteed to parse.
/// The alternative is a record that tears the two either side of it, which costs three
/// records instead of one.
///
/// The cut is by bytes, unlike the Swift original, which checked a byte count and then cut
/// by characters - so a record full of multi-byte text could still exceed the cap it was
/// being trimmed to meet.
fn truncated(line: &str) -> String {
    const SUFFIX: &str = "…\",\"truncated\":true}";
    let mut budget = MAXIMUM_RECORD_BYTES - SUFFIX.len();
    while !line.is_char_boundary(budget) {
        budget -= 1;
    }
    let mut out = String::with_capacity(MAXIMUM_RECORD_BYTES);
    out.push_str(&line[..budget]);
    out.push_str(SUFFIX);
    out
}

/// Nothing in a payload may break out of its line.
///
/// This log carries terminal bytes, which are full of quotes, escapes and control
/// characters. One unescaped newline splits a record into two unparseable halves - and it
/// would do so only for the records describing whatever went wrong, which are the ones
/// being read.
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    quote_into(&mut out, value);
    out
}

fn quote_into(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything else below 0x20 has no short form and must not go through raw.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
