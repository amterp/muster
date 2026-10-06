//! Muster's side of muster-daemon's protocol (MIP-3, section 11): the control connection the
//! app asks and follows a daemon on, the input connection its keystrokes travel, the stream a
//! bridge draws a pane from, and starting or adopting a daemon here or on another machine.

pub mod backend;
pub mod control;
pub mod convert;
pub mod follow;
pub mod handover;
pub mod input;
pub mod install;
pub mod launch;
pub mod records;
pub mod remote;
pub mod stream;

pub use muster_daemon_launch::{environment, silence_sigpipe};

// The client's own connections dial the way a start does, so the two share the one bounded
// handshake.
pub(crate) use muster_daemon_launch::{after_marker, dial, start_marker};
