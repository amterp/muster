//! What every pane Muster makes is handed, beyond what the daemon already gives it.
//!
//! Two kinds of entry. One is the same for every pane a daemon makes and exists to undo
//! something: Muster's daemon is pointed at a config file of Muster's own with
//! `HERDR_CONFIG_PATH`, and a pane's process inherits the daemon's environment - so without
//! this, `herdr` run inside a Muster pane would read Muster's derived file instead of the
//! user's own. Putting the user's path back on every pane-creating call is the whole of the fix.
//!
//! The other differs per pane: `MUSTER_PANE` is what the pane is called, and it is the only
//! thing that ever tells a program inside a pane which pane it is in. That is why this is
//! built per request rather than once at attach - the name is minted for the very request that
//! creates the pane, because the daemon's own id for it does not exist until the answer comes
//! back (see [`muster_core::names`]). `MUSTER_SOCKET` travels with it and says which Muster to
//! tell; between them they are the whole of how a program in a pane drives its own window.
//!
//! **Why this is a parameter rather than a scrub.** The alternative was to unset the variable
//! before spawning, which is a discipline nobody can verify from outside: the symptom of
//! forgetting is a `herdr` CLI reading the wrong file, which looks like nothing at all until
//! it does. As a parameter on each pane-creating intent it can be asserted - a conformance
//! case walks every intent Muster sends and fails any that could carry an environment and
//! does not.
//!
//! Two limits worth stating rather than chasing. A pane herdr restores after a daemon restart
//! is built with no launch environment at all (`persist/restore.rs`), and neither is one made
//! by a `herdr` client attached alongside Muster. Both read Muster's file. Muster can only
//! answer for the panes it makes.

use std::collections::BTreeMap;

use muster_core::mirror::backend::PaneId;
use serde_json::{Value, json};

use crate::discovery::config_file;

/// What a pane reads to find out which pane it is.
///
/// Renaming it breaks every pane already running, which is why it is a constant rather than a
/// literal at the one call site.
pub const PANE_NAME: &str = "MUSTER_PANE";

/// What a pane reads to find the window it is in.
///
/// The pair with [`PANE_NAME`], and useless without it: knowing which Muster to ask is only half
/// of being able to say "this pane". Set per window rather than looked up, because a machine can
/// have several Musters open and a pane belongs to exactly one - a program that went looking
/// would find several and have no way to tell which one is drawing it.
pub const WINDOW_SOCKET: &str = "MUSTER_SOCKET";

/// The environment entries a pane-creating request carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneEnvironment {
    entries: BTreeMap<String, String>,
}

impl PaneEnvironment {
    /// For a daemon whose config Muster did not write.
    ///
    /// A daemon named by `socket` in the config file is somebody else's, and a remote one is
    /// reached rather than started - Muster redirected neither, so neither has anything to
    /// restore. The request goes out exactly as it did before this existed, which is the
    /// honest wire for "nothing was changed here".
    pub fn none() -> PaneEnvironment {
        PaneEnvironment { entries: BTreeMap::new() }
    }

    /// For a daemon Muster started, and pointed at a config file of its own.
    ///
    /// Empty when the environment says nothing about where home is: there is then no user
    /// file to name, and sending an empty value would be worse than sending nothing at all.
    /// herdr sets a variable to empty rather than unsetting it, so a pane would end up
    /// looking for a config file called "" - a third behaviour, belonging to nobody.
    pub fn restoring(environment: &BTreeMap<String, String>) -> PaneEnvironment {
        let mut entries = BTreeMap::new();
        if let Some(path) = config_file(environment) {
            entries.insert("HERDR_CONFIG_PATH".to_string(), path);
        }
        PaneEnvironment { entries }
    }

    /// The same entries, plus the name the pane this request makes should call itself.
    ///
    /// A pane learns its own name exactly once, here. Nothing else can tell it: at the pinned
    /// herdr `pane.process_info` reports no tty, so a program inside a pane cannot work out
    /// which pane it is from the outside in.
    #[must_use]
    pub fn with_pane_name(&self, name: &PaneId) -> PaneEnvironment {
        let mut entries = self.entries.clone();
        entries.insert(PANE_NAME.to_string(), name.as_str().to_string());
        PaneEnvironment { entries }
    }

    /// The same entries, plus the endpoint of the window these panes will be in.
    ///
    /// Only for a daemon on this machine. A unix socket path means nothing on the far side of an
    /// ssh tunnel: a devenv pane handed one would either find no such file or, worse, find some
    /// unrelated one - so a remote pane is told nothing and its programs correctly conclude they
    /// are not in a Muster they can drive.
    #[must_use]
    pub fn reachable_at(&self, socket: &str) -> PaneEnvironment {
        let mut entries = self.entries.clone();
        entries.insert(WINDOW_SOCKET.to_string(), socket.to_string());
        PaneEnvironment { entries }
    }

    /// The `env` a pane-creating request carries, or nothing to put in one.
    pub fn as_params(&self) -> Option<Value> {
        if self.entries.is_empty() {
            return None;
        }
        Some(json!(self.entries))
    }
}
