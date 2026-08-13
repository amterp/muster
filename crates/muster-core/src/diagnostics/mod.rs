//! What Muster writes down about itself, so that "I just hit a bug" can be answered by
//! reading rather than by reproducing.
//!
//! Muster is several processes - the app, and one bridge per pane - and a symptom in one
//! usually has its cause in another. A window that ignores the keyboard can be a bridge
//! that never started, a bridge that started and could not dial back, or a keymap that
//! swallowed the chord, and from the outside those are the same blank stare. So the
//! processes write to one file in one timeline, and the questions become readable.

pub mod clock;
pub mod log;
pub mod sink;

pub use clock::{format_iso8601, monotonic_now, monotonic_since, wall_clock_millis};
pub use log::{LogLevel, LogRecord, LogSink};
pub use sink::JsonLinesSink;
