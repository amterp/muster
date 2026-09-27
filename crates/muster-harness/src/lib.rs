//! What a test needs to drive a real daemon: the daemon itself, waiting without sleeping, and a
//! relay that loses answers on request.
//!
//! Neutral between herdr and muster-daemon while both exist. `herdr-harness` spawns herdr and
//! re-exports `until` and the relay from here; the cut-over (MIP-3) deletes it, and this becomes
//! the harness.

mod control;
mod daemon;
mod input;
mod relay;
mod relay_daemon;
mod stream;
mod until;

pub use control::{Asked, Control};
pub use daemon::{DAEMON_DATA, Daemon, FIRST_ANSWER_BUDGET};
pub use input::Input;
pub use relay::{Holding, Pump, Relay};
pub use stream::Stream;
pub use until::{Detail, PATIENCE, until, until_file, until_some, until_within};
