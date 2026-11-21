//! Muster's bundled harness catalog, installation plans and hook formats.

mod agy;
mod claude_code;
mod codex;
mod opencode;

pub mod catalog;
pub mod hooks;
pub mod install;

/// The inbox transport for the untyped path and inode that Muster already keeps.
pub mod inbox {
    pub use crate::claude_code::inbox::{WAKE_FAILURE_CHECK, alive, deliver, discover, inode};
}
