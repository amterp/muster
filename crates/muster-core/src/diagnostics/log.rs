//! One record, the levels it can carry, and the process-wide switch that emits it.
//!
//! Records are one JSON object per line: greppable, and parseable without a tool.
//!
//! Off unless a sink is installed. `start_from_environment` decides that from
//! `MUSTER_LOG_FILE`, which the app sets for itself and every bridge it spawns.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};

use super::clock::{monotonic_now, wall_clock_millis};
use super::sink::JsonLinesSink;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Per-frame and per-keystroke volume. Off by default: at 60fps it buries everything
    /// that matters.
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn parse(name: &str) -> Option<LogLevel> {
        match name {
            "trace" => Some(LogLevel::Trace),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// One thing that happened.
///
/// `event` is a dotted name rather than a sentence - `bridge.attach.failed`, not "the
/// bridge could not attach" - so that finding every instance is a grep and not a guess at
/// how it was worded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// Milliseconds since the epoch, for a human lining this up against a wall clock.
    pub time_ms: i64,
    /// A machine-wide monotonic reading, so two records subtract even across processes.
    /// `time_ms` is for reading; this is for arithmetic.
    pub mono: u64,
    pub level: LogLevel,
    pub process: String,
    pub pid: i32,
    pub event: String,
    /// Sorted, so that two runs of the same code produce the same bytes.
    pub fields: BTreeMap<String, String>,
}

impl LogRecord {
    /// A record stamped with both clocks, now.
    pub fn now(
        level: LogLevel,
        process: impl Into<String>,
        pid: i32,
        event: impl Into<String>,
        fields: BTreeMap<String, String>,
    ) -> LogRecord {
        LogRecord {
            time_ms: wall_clock_millis(),
            mono: monotonic_now(),
            level,
            process: process.into(),
            pid,
            event: event.into(),
            fields,
        }
    }
}

pub trait LogSink: Send + Sync {
    fn write(&self, record: &LogRecord);

    /// Writes a line another process already wrote, newline included (see [`relay`]). A sink
    /// holding only its own process's records has nowhere to put one, and drops it.
    fn write_line(&self, _line: &str) {}
}

struct Installed {
    sink: Box<dyn LogSink>,
    minimum: LogLevel,
    process: String,
}

/// Where records go. Empty means logging is off, which is the release default.
///
/// Installed once during startup, before anything else runs, and read from every thread
/// after that.
static INSTALLED: RwLock<Option<Installed>> = RwLock::new(None);

/// Records emitted before a sink was installed, written when one is.
///
/// The app claims its lock and adopts old state before `Startup` installs the run's log, and what
/// those steps say is exactly what explains a launch that went wrong. Bounded, because a process
/// that never installs a sink - the CLI, a test - would otherwise keep every record it emits.
static EARLY: Mutex<Vec<LogRecord>> = Mutex::new(Vec::new());
const EARLY_LIMIT: usize = 256;
static INCLUDES_INPUT: OnceLock<bool> = OnceLock::new();

/// Recovers the log lock rather than reporting that it could not be taken.
///
/// The one lock in Muster that [`crate::diagnostics::poison`] cannot serve, because that
/// module reports a poisoned lock *by logging* and this is the lock it would take to do
/// it. So the recovery is spelled out here and stays silent.
///
/// Silence is the right answer rather than a compromise. A sink that panics mid-write - a
/// full disk, a closed file - poisons this lock, and the alternative is a process that has
/// lost the ability to say anything about itself at the exact moment it has something to
/// say. Losing one record beats losing the log (`README.md`, every run explains itself).
macro_rules! recovered {
    ($slot:expr) => {
        match $slot {
            Ok(guard) => guard,
            Err(poisoned) => {
                INSTALLED.clear_poison();
                poisoned.into_inner()
            }
        }
    };
}

/// Turns logging on for this process.
pub fn install(sink: Box<dyn LogSink>, process: impl Into<String>, minimum: LogLevel) {
    let process = process.into();
    let early = std::mem::take(&mut *EARLY.lock().unwrap_or_else(PoisonError::into_inner));
    for mut record in early.into_iter().filter(|record| record.level >= minimum) {
        record.process.clone_from(&process);
        sink.write(&record);
    }
    let mut slot = recovered!(INSTALLED.write());
    *slot = Some(Installed { sink, minimum, process });
}

/// Turns logging on if the environment asks for it.
///
/// `MUSTER_LOG_FILE` names the file; `MUSTER_LOG_LEVEL` raises or lowers the bar. The path
/// is chosen by the shell rather than here, because where logs belong is an OS question
/// and this layer does not get to have those.
pub fn start_from_environment(process: impl Into<String>) {
    let Ok(path) = std::env::var("MUSTER_LOG_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Some(sink) = JsonLinesSink::open(&path) else {
        return;
    };
    let level = std::env::var("MUSTER_LOG_LEVEL")
        .ok()
        .and_then(|name| LogLevel::parse(&name))
        .unwrap_or(LogLevel::Debug);
    install(Box::new(sink), process, level);
}

/// Whether records may carry what the user actually typed.
///
/// Off unless `MUSTER_LOG_INPUT=1`, and it stays that way in debug builds too. A log of
/// every keystroke is a keylogger no matter who wrote it, and this one lands in a file
/// that gets attached to bug reports. Call sites record the shape of input by default -
/// which key, how many bytes - and the bytes themselves only when this is on.
pub fn includes_input() -> bool {
    *INCLUDES_INPUT.get_or_init(|| std::env::var("MUSTER_LOG_INPUT").as_deref() == Ok("1"))
}

/// Whether anything would come of emitting at this level.
///
/// For call sites where building the fields is itself work worth skipping.
pub fn enabled(level: LogLevel) -> bool {
    let slot = recovered!(INSTALLED.read());
    slot.as_ref().is_some_and(|installed| level >= installed.minimum)
}

pub fn emit(level: LogLevel, event: &str, fields: BTreeMap<String, String>) {
    let slot = recovered!(INSTALLED.read());
    // SAFETY: getpid is always safe to call and reads no memory we own.
    let pid = unsafe { libc::getpid() };
    let Some(installed) = slot.as_ref() else {
        let mut early = EARLY.lock().unwrap_or_else(PoisonError::into_inner);
        if early.len() < EARLY_LIMIT {
            early.push(LogRecord::now(level, String::new(), pid, event, fields));
        }
        return;
    };
    if level < installed.minimum {
        return;
    }
    installed.sink.write(&LogRecord::now(level, &installed.process, pid, event, fields));
}

/// Appends a record a daemon wrote to this run's log, as the daemon wrote it.
///
/// A daemon outlives the run that started it, so it writes a log of its own and each run
/// follows it (MIP-3, section 1): its records arrive here already encoded. `daemon` names which
/// daemon wrote it, since a window may follow several. `received` is this machine's monotonic
/// clock when the record arrived, for a daemon on another machine: two machines' monotonic
/// clocks do not compare, so the record takes this one's and keeps its own as `daemon_mono_ns`.
///
/// Not filtered by this run's level: the daemon applied its own.
pub fn relay(line: &str, daemon: &str, received: Option<u64>) {
    let slot = recovered!(INSTALLED.read());
    let Some(installed) = slot.as_ref() else {
        return;
    };
    if let Some(mut line) = relabelled(line, daemon, received) {
        line.push('\n');
        installed.sink.write_line(&line);
        return;
    }
    // SAFETY: getpid is always safe to call and reads no memory we own.
    let pid = unsafe { libc::getpid() };
    let fields = crate::fields! {
        "daemon" => daemon,
        "line" => line,
        "impact" => "one of the daemon's records is in this log as text rather than as a \
                     record; the daemon's own file beside its socket has it whole",
        "check" => "whether the daemon and this app are the same build; this is likely \
                    a bug, since the daemon writes records in this format",
    };
    installed.sink.write(&LogRecord::now(
        LogLevel::Warn,
        &installed.process,
        pid,
        "daemon.log.unreadable",
        fields,
    ));
}

/// A daemon's record, naming the daemon, and with this machine's clock when `received` says.
/// None for a line that is not one of the records this log writes.
pub fn relabelled(line: &str, daemon: &str, received: Option<u64>) -> Option<String> {
    const CLOCK: &str = ",\"mono_ns\":";
    let body = line.trim_end().strip_prefix('{')?.strip_suffix('}')?;
    let body = match received {
        Some(received) => {
            let at = body.find(CLOCK)? + CLOCK.len();
            let digits = body[at..].bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let theirs = &body[at..at + digits];
            format!("{}{received},\"daemon_mono_ns\":{theirs}{}", &body[..at], &body[at + digits..])
        }
        None => body.to_string(),
    };
    Some(format!("{{{body},\"daemon\":{}}}", crate::diagnostics::sink::quote(daemon)))
}

/// Builds the field map from pairs, so a call site reads as a list rather than as
/// map plumbing.
///
/// A macro rather than a function taking an array, because the arity varies and the
/// values are nearly always formatted at the call site.
#[macro_export]
macro_rules! fields {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = ::std::collections::BTreeMap::<String, String>::new();
        $( map.insert(($key).to_string(), ($value).to_string()); )*
        map
    }};
}

macro_rules! at_level {
    ($name:ident, $level:expr) => {
        pub fn $name(event: &str, fields: BTreeMap<String, String>) {
            emit($level, event, fields);
        }
    };
}

at_level!(trace, LogLevel::Trace);
at_level!(debug, LogLevel::Debug);
at_level!(info, LogLevel::Info);
at_level!(warn, LogLevel::Warn);
at_level!(error, LogLevel::Error);

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{LogLevel, LogRecord, LogSink, emit, install, relabelled};

    struct Kept(Arc<Mutex<Vec<LogRecord>>>);

    impl LogSink for Kept {
        fn write(&self, record: &LogRecord) {
            self.0.lock().unwrap().push(record.clone());
        }
    }

    /// The only test in this binary that installs a sink, since a sink is the process's.
    #[test]
    fn what_was_said_before_the_log_was_installed_reaches_it() {
        emit(LogLevel::Info, "app.claimed", std::collections::BTreeMap::new());
        emit(LogLevel::Debug, "below.the.bar", std::collections::BTreeMap::new());
        let kept = Arc::new(Mutex::new(Vec::new()));
        install(Box::new(Kept(Arc::clone(&kept))), "app", LogLevel::Info);

        let written = kept.lock().unwrap();
        let claimed = written.iter().find(|record| record.event == "app.claimed");
        assert_eq!(claimed.map(|record| record.process.as_str()), Some("app"), "{written:?}");
        assert!(
            !written.iter().any(|record| record.event == "below.the.bar"),
            "a record below the installed level was written: {written:?}"
        );
    }

    const LINE: &str = "{\"time\":\"2026-09-27T03:00:00.000Z\",\"mono_ns\":5000,\"level\":\"info\",\
                        \"process\":\"daemon\",\"pid\":7,\"event\":\"pane.created\"}\n";

    #[test]
    fn a_daemons_record_says_which_daemon_wrote_it() {
        let line = relabelled(LINE, "devenv", None).unwrap();
        assert!(
            line.starts_with("{\"time\":") && line.ends_with(",\"daemon\":\"devenv\"}"),
            "{line}"
        );
        assert!(line.contains("\"mono_ns\":5000,"), "a local daemon's clock is this machine's");
    }

    /// Two machines' monotonic clocks do not compare, so a remote record takes the receipt time
    /// and keeps its own beside it.
    #[test]
    fn a_remote_record_takes_this_machines_clock() {
        let line = relabelled(LINE, "devenv", Some(99)).unwrap();
        assert!(line.contains("\"mono_ns\":99,\"daemon_mono_ns\":5000,\"level\""), "{line}");
    }

    #[test]
    fn a_line_that_is_not_a_record_is_not_relabelled() {
        assert_eq!(relabelled("not json", "d", None), None);
        assert_eq!(relabelled("{\"time\":\"x\"}", "d", Some(1)), None, "no clock to replace");
    }
}
