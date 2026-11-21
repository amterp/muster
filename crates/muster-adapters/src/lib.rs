//! The harness formats used by Muster's inbox and hook commands.

mod claude_code;
mod codex;

pub mod hooks;

/// The inbox transport for the untyped path and inode that Muster already keeps.
pub mod inbox {
    pub use crate::claude_code::inbox::{WAKE_FAILURE_CHECK, alive, deliver, discover, inode};
}
