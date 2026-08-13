//! The terminal Muster reasons with rather than shows.
//!
//! libghostty-vt: the same engine the renderer runs, headless. The input path encodes keys
//! with it and tests read grids from it. Nothing here needs a GPU, a window, or a running
//! app.

mod ffi;
mod key_encoder;
mod key_mapping;

pub use key_encoder::{EncoderError, KeyEncoder};
