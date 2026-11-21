//! How to interpret the daemon's adapter report for a pane.

use crate::mirror::backend::Adapter;

pub use muster_adapters::catalog;
pub use muster_adapters::install::extras_beside;

/// Whether a pane's agent reports through an adapter, with what Muster can do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Nothing to say yet: no agent, or one that has not ended a turn.
    Unsaid,
    Reporting,
    /// It ended a turn without reporting, and Muster ships an adapter for its harness.
    Silent(&'static str),
    /// It ended a turn without reporting, and Muster ships no adapter for its harness.
    None,
}

impl Standing {
    /// How `muster window --json` spells it, empty for nothing to say.
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Unsaid => "",
            Standing::Reporting => "reporting",
            Standing::Silent(_) => "silent",
            Standing::None => "none",
        }
    }
}

/// What the daemon's word on a pane's adapter means, for the agent it found there.
pub fn standing(agent: Option<&str>, adapter: Adapter) -> Standing {
    match adapter {
        Adapter::Unsaid => Standing::Unsaid,
        Adapter::Reporting => Standing::Reporting,
        Adapter::Silent => match agent.and_then(catalog::named) {
            Some(harness) => Standing::Silent(harness.dir),
            None => Standing::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_bundle_keeps_its_adapters_in_its_resources() {
        assert_eq!(
            extras_beside(Path::new("/Applications/Muster.app/Contents/MacOS/muster-cli")),
            Path::new("/Applications/Muster.app/Contents/Resources/extras")
        );
        assert_eq!(
            extras_beside(Path::new("/home/me/.muster/daemon/0.12.0/muster")),
            Path::new("/home/me/.muster/daemon/0.12.0/extras")
        );
    }

    #[test]
    fn silence_from_a_harness_without_an_adapter_is_nothing_to_install() {
        assert_eq!(standing(Some("gemini"), Adapter::Silent), Standing::None);
        assert_eq!(standing(Some("codex"), Adapter::Silent), Standing::Silent("codex"));
        assert_eq!(standing(Some("codex"), Adapter::Unsaid).as_str(), "");
    }
}
