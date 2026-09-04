//! Muster's headless core: what a keystroke means, what an agent is doing, what happened.
//!
//! No OS types, no windowing, no rendering. The shell above decides *that* something
//! happened and this decides what it means (MIP-1, `docs/architecture.md`). Everything
//! here is reachable by a test without a daemon, a window, or a running app, and the
//! cases it answers to live in `corpus/conformance/` rather than in this language.

pub mod agent_state;
pub mod attention;
pub mod composition;
pub mod config;
pub mod daemons;
pub mod diagnostics;
pub mod find;
pub mod font;
pub mod input;
pub mod intent;
pub mod mirror;
pub mod names;
pub mod problems;
pub mod reconnect;
pub mod respawn;
pub mod roster;
pub mod typeable;

pub use agent_state::AgentState;
pub use attention::{Alert, Attend, Attention, Noticed, Notifications};
pub use composition::{Composition, PaneKey};
pub use config::Config;
pub use daemons::Started;
pub use find::{Found, Hit, Needle, found_in};
pub use input::{Key, KeyEvent, Keymap, Modifiers, PaneInput};
pub use intent::{BackendChannel, BackendIntent};
pub use mirror::{BackendEvent, Change, Mirror};
pub use names::{BackendPaneId, BackendTabId, Mint, Names, PaneNames, TabNames};
pub use problems::{Problem, Problems, Severity};
pub use respawn::Respawns;
pub use roster::{Landing, Numbering, Roster, RosterPane};
pub use typeable::Waiting;
