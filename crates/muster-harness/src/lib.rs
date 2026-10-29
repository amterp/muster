//! What a test needs to drive a real daemon: the daemon itself, waiting without sleeping, a
//! relay that loses answers on request, a fake agent the daemon detects, and a client of the API.

mod agents;
mod api_client;
mod control;
mod daemon;
mod input;
mod relay;
mod relay_daemon;
pub mod requests;
mod stream;
mod until;

pub use api_client::{Api, ApiAsked, Tried, new_pane, subscribe_to, test_client};
pub use control::{Asked, Control};
pub use daemon::{
    DAEMON_DATA, Daemon, FIRST_ANSWER_BUDGET, Replacing, built_daemon, built_linux_daemons,
};
pub use input::Input;
pub use relay::{Holding, Pump, Relay};
pub use stream::Stream;
pub use until::{Detail, PATIENCE, until, until_file, until_some, until_within};
