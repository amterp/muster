//! The shell/core seam: one symbol in, one callback out.
//!
//! The shell and the core are different languages in one process, so the boundary between
//! them is real and has to be narrow (architecture.md, the shell/core seam). Everything
//! that crosses it is a protobuf message in Muster's own vocabulary, and this crate is the
//! only place that knows that - `muster-core` stays free of both protobuf and FFI, so the
//! conformance corpus keeps judging the same code whether or not a shell is attached.
//!
//! The exported functions are declared in `include/muster.h`, which is hand-written
//! because it is the contract.
//!
//! One symbol in, and now also one socket: `command` answers the same `Request` from outside
//! the process, through the same [`dispatch`]. The seam is where a transport is turned into a
//! request, so both transports belong here and neither is a second path.

// Public because the exported symbols are this crate's whole surface, even though no Rust
// caller reaches them - `unreachable_pub` cannot see through `extern "C"`.
pub mod command;
mod convert;
pub mod ffi;
mod handler;
pub mod proto;
mod session;

pub use ffi::emit;
pub use handler::dispatch;
