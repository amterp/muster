//! Muster's protocol for muster-daemon, shared by the daemon and every client of it.
//!
//! The daemon and the app are separately built programs that routinely differ in version, since
//! an app adopts whichever daemon is running (MIP-3, section 9). So this crate holds what the
//! two must agree on and nothing else: the generated messages (`proto/muster_daemon.proto`,
//! whose header is the protocol's documentation), the version rule, where an install's daemon
//! listens, and the handshake that opens every connection.

pub mod connection;
pub mod install;
pub mod version;

include!(concat!(env!("OUT_DIR"), "/muster.daemon.rs"));
