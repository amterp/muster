//! What a test needs to drive a real daemon: waiting without sleeping, and a relay that loses
//! answers on request.
//!
//! Neutral between the two daemons while both exist. `herdr-harness` spawns herdr and re-exports
//! this; the cut-over (MIP-3) deletes it.

mod relay;
mod until;

pub use relay::{Holding, Pump, Relay};
pub use until::{Detail, PATIENCE, until, until_file, until_some, until_within};
