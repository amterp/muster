//! The core's picture of what a daemon holds, and the vocabulary it is written in.
//!
//! Three files, split by what each is allowed to know. `backend` names the things - tabs and
//! panes - in the core's own types. `event` names what can change about them. `state` folds
//! the second into the first.

pub mod backend;
pub mod event;
pub mod ordered;
pub mod state;

pub use backend::{AgentFacts, Health, Pane, PaneId, Snapshot, Tab, TabId};
pub use event::{BackendEvent, Change, Restored};
pub use state::Mirror;
