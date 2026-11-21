//! Decoders for the existing hook flags.

use crate::{claude_code, codex};

pub use claude_code::hooks::is_main_agent;
pub use claude_code::transcript::recorded_compaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    Cleared,
    Started,
    Other,
}

/// The shipped `--session-source` flag accepts either hook's spelling independently of `--agent`.
pub fn session_source(source: &str) -> SessionSource {
    match source {
        claude_code::hooks::CLEAR_SOURCE => SessionSource::Cleared,
        codex::STARTUP_SOURCE => SessionSource::Started,
        _ => SessionSource::Other,
    }
}
