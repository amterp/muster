//! Agents' messages to each other (MIP-4): who takes part, which groups they are in, each group's
//! log, what each participant has read, and whom a new message should wake.
//!
//! Everything here is a function of that state and what a caller asks. How to reach a
//! participant, whether it is still there, and where the logs are kept belong to whatever hosts
//! this - muster-daemon, over its sockets and its disk - and reach it through [`Store`] and
//! [`Presence`], or leave it as a [`Wake`] for the host to deliver. So a pane is an address the
//! crate knows only by name, as it knows an inbox only by path; it names no tab or window, and
//! its tests run with no daemon.

mod entry;
mod names;
mod policy;
mod refusal;
mod service;
mod store;

pub use entry::{Entry, What};
pub use names::{HUMAN, LONGEST_GROUP, default_name, pair_group};
pub use policy::Policy;
pub use refusal::Refusal;
pub use service::{
    Activity, AnsweredWait, Caller, Doorbell, Inbox, Joined, Left, Liveness, Member, Messaging,
    Notice, Participant, Posted, Presence, Reach, Read, Via, Waited, Wake,
};
pub use store::{GroupRecord, Memory, Saved, Store};

/// The largest message body, in bytes. A daemon frame may be 16 MiB, and a message is meant to
/// be read by a model, so this leaves room for everything else a frame carries.
pub const LARGEST_BODY: usize = 1024 * 1024;
