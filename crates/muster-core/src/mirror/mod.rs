//! The core's picture of what a backend holds, and the vocabulary it is written in.
//!
//! Three files, split by what each is allowed to know. `backend` names the things -
//! workspaces, tabs, panes, focus - in Muster's terms rather than any daemon's. `event`
//! names what can change about them. `state` folds the second into the first.
//!
//! Nothing herdr-shaped appears in any of them: an adapter translates into this
//! vocabulary, and a second backend would translate into the same one
//! (`docs/architecture.md`, the vocabulary).

pub mod backend;
pub mod event;
pub mod ordered;
pub mod state;

pub use backend::{Focus, Health, Pane, PaneId, Snapshot, Tab, TabId, Workspace, WorkspaceId};
pub use event::{BackendEvent, Change};
pub use state::Mirror;
