//! One record, the levels it can carry, and the process-wide switch that emits it.
//!
//! Records are one JSON object per line: greppable, and parseable without a tool.
//!
//! Off unless a sink is installed. `start_from_environment` decides that from
//! `MUSTER_LOG_FILE`, which the app sets for itself and every bridge it spawns.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

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
static INCLUDES_INPUT: OnceLock<bool> = OnceLock::new();

/// Turns logging on for this process.
pub fn install(sink: Box<dyn LogSink>, process: impl Into<String>, minimum: LogLevel) {
    let mut slot = INSTALLED.write().expect("the log lock was poisoned by a panicking writer");
    *slot = Some(Installed { sink, minimum, process: process.into() });
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
    let slot = INSTALLED.read().expect("the log lock was poisoned by a panicking writer");
    slot.as_ref().is_some_and(|installed| level >= installed.minimum)
}

pub fn emit(level: LogLevel, event: &str, fields: BTreeMap<String, String>) {
    let slot = INSTALLED.read().expect("the log lock was poisoned by a panicking writer");
    let Some(installed) = slot.as_ref() else {
        return;
    };
    if level < installed.minimum {
        return;
    }
    // SAFETY: getpid is always safe to call and reads no memory we own.
    let pid = unsafe { libc::getpid() };
    installed.sink.write(&LogRecord::now(level, &installed.process, pid, event, fields));
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
