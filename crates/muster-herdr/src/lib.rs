//! The one herdr-shaped module.
//!
//! Everything that knows herdr's wire format lives here, so that swapping the session
//! backend is a matter of writing a second one of these rather than of finding every place
//! a JSON key leaked (architecture.md, the backend seam). The core above speaks Muster's
//! own vocabulary and does not know this crate exists.

pub mod control_stream;
pub mod discovery;
pub mod frame;

pub use control_stream::ControlStreamMessage;
pub use discovery::discover_socket_path;
pub use frame::{FrameDecoder, PaneFrame, PaneStreamEvent};
