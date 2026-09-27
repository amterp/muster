//! The terminal Muster reasons with rather than shows.
//!
//! libghostty-vt: the same engine the renderer runs, headless. The input path encodes keys
//! with it and tests read grids from it. Nothing here needs a GPU, a window, or a running
//! app.

mod build_info;
mod cells;
mod ffi;
mod formatter;
mod grid;
mod key_encoder;
mod key_mapping;
mod modes;
mod replay;
mod state;
mod terminal;

pub use build_info::engine_version;
pub use formatter::{Extras, Format, FormatOptions, ScreenExtras, ScreenFormatOptions, Selection};
pub use grid::{Cell, Color, Cursor, Grid, Row, Style, Width};
pub use key_encoder::{EncoderError, KeyEncoder};
pub use modes::Mode;
pub use state::{Palette, Rgb, Screen};
pub use terminal::{Terminal, TerminalError};
