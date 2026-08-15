//! One ssh master per remote daemon, and the local socket path it forwards.
//!
//! The whole of "local and remote in one window" at the transport layer. A remote herdr
//! speaks the same JSON socket a local one does, so forwarding that socket to a path on this
//! machine leaves every layer above unable to tell the difference: the client, the snapshot,
//! the subscription, the agent watchers and the server-side encoder all take a socket path
//! and none of them inspects it. That is the bet, and `corpus/herdr-0.8.0-linux` is the
//! evidence for it - the same recordings against a Linux daemon differ in nothing
//! (`docs/observations/herdr-0.8.0.md` section 8).
//!
//! Transport only, with nothing herdr-shaped in it. What lives on the far end of the socket
//! is the adapter's business; what this owns is a child process, a path, and the promise that
//! the path keeps working.
//!
//! The data plane cannot use the same trick. A pane's frames come from `herdr terminal
//! session control`, which is a CLI over stdio rather than a socket method, so the bridge
//! runs that command through this master instead - which is why the control path is public.

mod tunnel;

pub use tunnel::{Forward, Tunnel, master_arguments, remote_environment};
