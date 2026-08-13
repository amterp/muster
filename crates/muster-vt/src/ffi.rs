//! libghostty-vt's C API, as bindgen sees it.
//!
//! Generated at build time from the pinned header - see `build.rs`. Nothing outside this
//! crate touches these names: the seam is `KeyEncoder`, and the core above does not know
//! libghostty-vt exists.

// Generated code answers to the C header, not to our style. Every lint the workspace turns
// on is turned back off here, so that a warning in this crate is always about code someone
// wrote.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
#![allow(dead_code, unreachable_pub, missing_debug_implementations, unused_qualifications)]
#![allow(clippy::all, clippy::pedantic)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
