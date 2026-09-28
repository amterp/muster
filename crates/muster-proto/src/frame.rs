//! How a message is carried over the command socket.
//!
//! The framing is `muster-frame`'s, shared with the daemon's protocol. What is this protocol's
//! own is the ceiling below, and the shape of a conversation: one request per connection,
//! answered once - or, for a `WatchPanes`, answered frame after frame until one end hangs up -
//! so which end is talking is decided by who dialed.
//!
//! Re-exported here so that the CLI and the window reach the framing through the schema they
//! already share. The CLI is built from this repo but runs as whatever version somebody has on
//! their PATH, so the framing has to be something both sides agree on without negotiating.

pub use muster_frame::{read_frame, read_frame_or_end, write_frame};

/// The most a message either way may be.
///
/// Every request Muster has is a few hundred bytes and the largest imaginable is a paste. The
/// largest answer is a pane's text: a whole read is one page from its daemon, up to 4 MiB, so
/// this is twice that, and a history longer than the limit reaches `muster pane read` rather
/// than being refused as a schema mismatch. It is here so that a caller who is not Muster's CLI -
/// a port scanner, a truncated write, a client built against a different schema - cannot make
/// the app reserve a gigabyte by claiming to be about to send one, and so that the CLI is
/// protected the same way from the same mistake.
pub const LARGEST_MESSAGE: u32 = 8 << 20;
