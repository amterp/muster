//! What every pane Muster makes is handed, beyond what the daemon already gives it.
//!
//! There is one entry today and it exists to undo something. Muster's daemon is pointed at a
//! config file of Muster's own with `HERDR_CONFIG_PATH`, and a pane's process inherits the
//! daemon's environment - so without this, `herdr` run inside a Muster pane would read
//! Muster's derived file instead of the user's own. Putting the user's path back on every
//! pane-creating call is the whole of the fix.
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

use serde_json::{Value, json};

use crate::discovery::config_file;

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

    /// The `env` a pane-creating request carries, or nothing to put in one.
    pub fn as_params(&self) -> Option<Value> {
        if self.entries.is_empty() {
            return None;
        }
        Some(json!(self.entries))
    }
}
