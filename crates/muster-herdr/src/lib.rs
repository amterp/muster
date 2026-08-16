//! The one herdr-shaped module.
//!
//! Everything that knows herdr's wire format lives here, so that swapping the session
//! backend is a matter of writing a second one of these rather than of finding every place
//! a JSON key leaked (architecture.md, the backend seam). The core above speaks Muster's
//! own vocabulary and does not know this crate exists.

pub mod client;
pub mod config;
pub mod control_socket;
pub mod control_stream;
pub mod daemon;
pub mod discovery;
pub mod env;
pub mod events;
pub mod frame;
pub mod intent;
pub mod layout;
pub mod pane_channel;
pub mod snapshot;
pub mod subscription;

pub use client::HerdrClient;
pub use config::{herdr_configuration, write_configuration};
pub use control_socket::PaneControlChannel;
pub use control_stream::ControlStreamMessage;
pub use discovery::{discover_socket_path, own_socket_path};
pub use env::PaneEnvironment;
pub use events::EventDecoder;
pub use frame::{FrameDecoder, PaneFrame, PaneStreamEvent};
pub use intent::{HerdrBackend, refusal, request};
pub use layout::{read_exported_layout, read_layout};
pub use pane_channel::HerdrPaneChannel;
pub use snapshot::{fetch_snapshot, read_snapshot};
pub use subscription::{Notice, Subscription};
