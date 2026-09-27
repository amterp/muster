//! Muster's side of muster-daemon's protocol (MIP-3, section 11): the control connection the
//! app asks and follows a daemon on, the input connection its keystrokes travel, and the stream
//! a bridge draws a pane from.

pub mod control;
pub mod stream;
