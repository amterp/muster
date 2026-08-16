//! What Muster itself decided: which daemons are attached, and what each region shows.
//!
//! The only thing Muster owns rather than mirrors, and so the only thing worth writing
//! down: everything else it holds is derived from a daemon and can be asked for again
//! (`docs/architecture.md`, durability). That is why `record` is records and nothing else -
//! no closures, no handles, no view objects, nothing a file cannot hold. It is a few
//! hundred bytes, and its smallness is the point.
//!
//! It is also the one piece nobody else can save. A daemon's own export is scoped to
//! itself and structurally cannot describe a window spanning a laptop and a devenv, because
//! neither daemon knows the other exists.
//!
//! Regions sit in a list rather than a tree. A region shows one tab, whose pane tree is
//! daemon truth; Muster owns no outer tree over those, because owning one is what would
//! make it a multiplexer (a non-goal). Side by side, in this order, is the whole
//! arrangement.
//!
//! One window's worth of regions, because there is one window. A second window wraps the
//! list and leaves the daemons where they are - which daemons are attached is not a
//! window's business.
//!
//! `saved` is what that smallness buys: the records write to a few hundred bytes of TOML and
//! read back, so a window reopens onto the tabs it was showing, in the order and at the
//! widths somebody put them in. It writes intent and never observation - the tab that was
//! chosen, not what was in it - so what comes back is a wish that gets checked against what
//! the daemons turn out to hold.
//!
//! `view` is the other half: what those records plus a daemon's mirror add up to on screen.
//! Kept apart because they are owned differently - the records are Muster's decisions and
//! survive a restart, and a view is derived, disposable, and correct only for as long as
//! the mirror behind it is.

pub mod presentation;
pub mod record;
pub mod saved;
pub mod view;

pub use presentation::{FontSizeChange, Presentation};
pub use record::{Composition, Daemon, DaemonId, Endpoint, PaneKey, Region, RegionId, TabKey};
pub use saved::{Restorable, Saved, SavedRegion};
pub use view::{Step, Transport, View, ViewNode, ViewPane, ViewRegion};
