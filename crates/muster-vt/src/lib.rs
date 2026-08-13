//! The terminal Muster reasons with rather than shows.
//!
//! libghostty-vt: the same engine the renderer runs, headless. The input path encodes keys
//! with it and tests read grids from it. Nothing here needs a GPU, a window, or a running
//! app.

mod ffi;
mod grid;
mod key_encoder;
mod key_mapping;
mod terminal;

pub use grid::{Cell, Cursor, Grid, Row, Width};
pub use key_encoder::{EncoderError, KeyEncoder};
pub use terminal::{Terminal, TerminalError};
