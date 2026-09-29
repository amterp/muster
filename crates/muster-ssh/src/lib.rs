//! One ssh master per remote daemon, and the local socket path it forwards.
//!
//! The whole of "local and remote in one window" at the transport layer. A remote daemon
//! speaks the same protocol on the same kind of socket a local one does, so forwarding that
//! socket to a path on this machine leaves every layer above unable to tell the difference:
//! the follower, every request and every pane's bridge take a socket path and none of them
//! inspects it. The bet was made against herdr, where the same recordings against a Linux
//! daemon differed in nothing (`docs/observations/herdr-0.8.0.md` section 8), and
//! `./dev --ssh` holds muster-daemon to it.
//!
//! Transport only, with nothing daemon-shaped in it. What lives on the far end of the socket
//! is the adapter's business; what this owns is a child process, a path, and the promise that
//! the path keeps working.
//!
//! [`Remote`] is the same connection put to a third use: running a command on the far machine
//! and copying a file to it, so that Muster can put its own daemon over there rather than
//! attaching whatever somebody installed. What it copies and what it runs are the caller's
//! business - this crate stays a child process, a path, and the promise that the path keeps
//! working.
//!
//! [`Reverse`] is the connection's other direction: a socket on this machine made to answer at a
//! path over there, which is how a program on the far machine reaches the window that drew it.

mod left_behind;
mod remote;
mod tunnel;

pub use left_behind::{end_left_behind, tunnel_path};
pub use remote::{Platform, Remote, quoted};
pub use tunnel::{
    Forward, Report, Reverse, State, Tunnel, master_arguments, remote_environment,
    reverse_arguments,
};
