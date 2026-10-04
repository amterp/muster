//! The harnesses Muster ships an adapter for, and where those adapters are on this machine
//! (MIP-5). An adapter is the hooks or plugin in `extras/<dir>/` that has a harness report
//! itself to the daemon; the daemon only says whether a pane's agent reports, and this is what
//! knows whether there is anything to install when it does not.

use std::path::{Path, PathBuf};

use crate::mirror::backend::Adapter;

/// A harness with an adapter in `extras/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Harness {
    /// Its detection manifest's id, as `muster window` and `report --agent` name it.
    pub id: &'static str,
    /// Its adapter's directory under `extras/`, which is also what a person types for it.
    pub dir: &'static str,
    /// What a person calls it.
    pub name: &'static str,
}

/// Every harness Muster ships an adapter for. A test holds this to `extras/`.
pub const WITH_ADAPTERS: [Harness; 3] = [
    Harness { id: "claude", dir: "claude-code", name: "Claude Code" },
    Harness { id: "codex", dir: "codex", name: "Codex" },
    Harness { id: "opencode", dir: "opencode", name: "OpenCode" },
];

/// The harness a person or a manifest named, by its adapter's directory or its manifest id.
pub fn named(name: &str) -> Option<Harness> {
    WITH_ADAPTERS.into_iter().find(|harness| harness.dir == name || harness.id == name)
}

/// Whether a pane's agent reports through an adapter, with what Muster can do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Nothing to say yet: no agent, or one that has not ended a turn.
    Unsaid,
    Reporting,
    /// It ended a turn without reporting, and Muster ships an adapter for its harness.
    Silent(Harness),
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
        Adapter::Silent => match agent.and_then(named) {
            Some(harness) => Standing::Silent(harness),
            None => Standing::None,
        },
    }
}

/// Where the adapters ship beside an executable of Muster's: a bundle keeps them in its
/// `Resources`, and every other layout - `./dev`'s staging, an install on an SSH machine - beside
/// the executable itself.
pub fn extras_beside(executable: &Path) -> PathBuf {
    let directory = executable.parent().unwrap_or(Path::new("/"));
    match directory.parent() {
        Some(contents)
            if directory.file_name().is_some_and(|name| name == "MacOS")
                && contents.file_name().is_some_and(|name| name == "Contents") =>
        {
            contents.join("Resources").join("extras")
        }
        _ => directory.join("extras"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_harness_is_named_by_its_directory_or_its_manifest() {
        assert_eq!(named("claude-code"), named("claude"));
        assert_eq!(named("claude").map(|harness| harness.dir), Some("claude-code"));
        assert_eq!(named("gemini"), None);
    }

    #[test]
    fn silence_from_a_harness_without_an_adapter_is_nothing_to_install() {
        assert_eq!(standing(Some("gemini"), Adapter::Silent), Standing::None);
        assert_eq!(standing(Some("codex"), Adapter::Silent), Standing::Silent(WITH_ADAPTERS[1]));
        assert_eq!(standing(Some("codex"), Adapter::Unsaid).as_str(), "");
    }

    /// Every adapter directory in `extras/` has a row, and every row a directory, so the table
    /// cannot drift from what ships.
    #[test]
    fn the_table_is_what_extras_holds() {
        let extras = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extras");
        let mut shipped: Vec<String> = std::fs::read_dir(&extras)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.') && name != "skill")
            .collect();
        shipped.sort();
        let mut listed: Vec<String> =
            WITH_ADAPTERS.iter().map(|harness| harness.dir.to_string()).collect();
        listed.sort();
        assert_eq!(shipped, listed, "extras/ and harnesses::WITH_ADAPTERS disagree");
    }
}
