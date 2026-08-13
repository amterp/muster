//! What the drivers share, so that one corpus spelling means one thing.
//!
//! Split by what it is for: `input` fakes the channels and encoder the input path needs,
//! `backend` reads a daemon's world out of a case. Both are part of the contract rather
//! than incidental scaffolding - `pane-input.json` states what the fakes do, and every
//! corpus that describes a session describes it in the shape `backend` reads - so a driver
//! in another language builds the same ones.
//!
//! Each driver is its own binary and uses one slice of this, so whatever the binary being
//! compiled does not touch is dead to it. That is a property of how Rust builds integration
//! tests, not a sign that something here has no readers.
#![allow(dead_code)]

pub(crate) mod backend;
pub(crate) mod input;
