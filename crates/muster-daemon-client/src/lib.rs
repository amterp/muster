//! Muster's side of muster-daemon's protocol (MIP-3, section 11).
//!
//! Today this is the stream half, which the bridge uses to draw a pane. The control and input
//! halves, which the seam will use, arrive when the seam moves onto the daemon.

pub mod stream;
