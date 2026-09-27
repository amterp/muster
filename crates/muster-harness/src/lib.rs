//! What a test needs to drive a real daemon: the daemon itself, waiting without sleeping, a
//! relay that loses answers on request, and a fake agent the daemon detects.

mod agents;
mod control;
mod daemon;
mod input;
mod relay;
mod relay_daemon;
pub mod requests;
mod stream;
mod until;

pub use control::{Asked, Control};
pub use daemon::{DAEMON_DATA, Daemon, FIRST_ANSWER_BUDGET, Replacing, built_daemon};
pub use input::Input;
pub use relay::{Holding, Pump, Relay};
pub use stream::Stream;
pub use until::{Detail, PATIENCE, until, until_file, until_some, until_within};
