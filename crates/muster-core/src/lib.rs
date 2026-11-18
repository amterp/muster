//! Muster's headless core: what a keystroke means, what an agent is doing, what happened.
//!
//! The core has no OS types or windowing (MIP-1, `docs/architecture.md`). The shell reports
//! what happened, and the core decides what it means. The core can be tested without a
//! daemon, window, or running app. Its cases live in `corpus/conformance/`.

pub mod agent_state;
pub mod attention;
pub mod banners;
pub mod bridge_link;
pub mod callsigns;
pub mod canvas;
pub mod compaction;
pub mod composition;
pub mod config;
pub mod conversation;
pub mod daemon_settings;
pub mod daemons;
pub mod diagnostics;
pub mod equalize;
pub mod focus_history;
pub mod font;
pub mod harnesses;
pub mod helpers;
pub mod input;
pub mod intent;
pub mod making;
pub mod markdown;
pub mod message_groups;
pub mod mirror;
pub mod names;
pub mod painting;
pub mod pane_address;
pub mod pane_focus;
pub mod pane_text;
pub mod posting;
pub mod presence;
pub mod problems;
pub mod reconnect;
pub mod resize;
pub mod respawn;
pub mod roster;
pub mod spanning;
pub mod typeable;

pub use agent_state::{AgentState, Until};
pub use attention::{Alert, Asker, Attend, Attention, GroupKey, Noticed, Notifications};
pub use composition::{Composition, PaneKey};
pub use config::Config;
pub use daemons::Started;
pub use equalize::{Divider, Evenly};
pub use input::{Key, KeyEvent, Keymap, Modifiers, PaneInput};
pub use intent::{BackendChannel, BackendIntent};
pub use mirror::{BackendEvent, Change, Mirror};
pub use names::{Mint, Minter};
pub use painting::Painting;
pub use problems::{Problem, Problems, Severity};
pub use respawn::Respawns;
pub use roster::{Landing, Numbering, Roster, RosterPane};
pub use typeable::Waiting;
