//! Every agent Muster can recognise: the built-in manifests, the app's, and a person's
//! overrides, layered in that order.
//!
//! Ported from herdr v0.8.0 `src/detect/manifest.rs` (the loader) and `src/detect/mod.rs` (the
//! agent names; Apache-2.0), and changed: herdr's agents are a compiled enum with a name table
//! beside it, and its layers are bundled, a remote catalog cache and an override directory.
//! Here an agent is its manifest and answers to the manifest's id and aliases, and the middle
//! layer is whatever the app sent at connect.
//!
//! So an agent with no manifest is not an agent. herdr also recognised `omp` and `mastracode`,
//! whose states its own plugins reported over hook authority, which Muster does not port.
//! Recognised here with no rules, they would read idle while they worked, so they stay
//! unknown; a manifest in the override directory names them for whoever wants the name shown.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::manifest::{ENGINE_VERSION, Manifest};
use crate::{Agent, Detection, Input, State};

const BUILT_IN: &[(&str, &str)] = &[
    ("amp.toml", include_str!("../manifests/amp.toml")),
    ("antigravity.toml", include_str!("../manifests/antigravity.toml")),
    ("claude.toml", include_str!("../manifests/claude.toml")),
    ("cline.toml", include_str!("../manifests/cline.toml")),
    ("codex.toml", include_str!("../manifests/codex.toml")),
    ("cursor.toml", include_str!("../manifests/cursor.toml")),
    ("devin.toml", include_str!("../manifests/devin.toml")),
    ("droid.toml", include_str!("../manifests/droid.toml")),
    ("gemini.toml", include_str!("../manifests/gemini.toml")),
    ("github-copilot.toml", include_str!("../manifests/github-copilot.toml")),
    ("grok.toml", include_str!("../manifests/grok.toml")),
    ("hermes.toml", include_str!("../manifests/hermes.toml")),
    ("kilo.toml", include_str!("../manifests/kilo.toml")),
    ("kimi.toml", include_str!("../manifests/kimi.toml")),
    ("kiro.toml", include_str!("../manifests/kiro.toml")),
    ("maki.toml", include_str!("../manifests/maki.toml")),
    ("opencode.toml", include_str!("../manifests/opencode.toml")),
    ("pi.toml", include_str!("../manifests/pi.toml")),
    ("qodercli.toml", include_str!("../manifests/qodercli.toml")),
];

/// Where a manifest in use came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    /// Sent by the app, under the name it gave.
    App(String),
    Override(PathBuf),
}

impl Source {
    fn rank(&self) -> u8 {
        match self {
            Source::BuiltIn => 0,
            Source::App(_) => 1,
            Source::Override(_) => 2,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::BuiltIn => f.write_str("the built-in manifest"),
            Source::App(name) => write!(f, "the app's manifest {name}"),
            Source::Override(path) => write!(f, "the override {}", path.display()),
        }
    }
}

/// A manifest that was offered and not used, worded to be logged as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub source: Source,
    pub problem: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ignored {}: {}. ", self.source, self.problem)?;
        match &self.source {
            Source::Override(_) => f.write_str(
                "That agent is detected by the manifest it had before, if any. Correct or \
                 remove the file; the directory is read again whenever the manifests are.",
            ),
            Source::App(_) => f.write_str(
                "That agent is detected by the manifest the daemon already had. An app and \
                 daemon from the same release always agree, so this is a version skew.",
            ),
            Source::BuiltIn => f.write_str("This is a bug in Muster's build."),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    agent: Agent,
    manifest: Manifest,
    source: Source,
    /// The manifest as written, which is what decides whether two loads differ for an agent.
    text: Arc<str>,
}

/// The agents a daemon recognises, and the rules for each.
#[derive(Debug, Clone)]
pub struct Manifests {
    entries: Vec<Entry>,
    /// Every name an agent answers to, normalized, to its entry.
    names: HashMap<String, usize>,
    /// Each entry's script paths, split into normalized components.
    script_paths: Vec<(Vec<String>, usize)>,
}

impl Manifests {
    /// The manifests compiled into this crate.
    pub fn built_in() -> Manifests {
        let entries = BUILT_IN
            .iter()
            .map(|(file, text)| {
                // The manifests are fixed at build time and a test parses every one, so a
                // failure here is this crate's own bug, not an input to handle.
                let manifest = Manifest::parse(text)
                    .unwrap_or_else(|error| panic!("built-in manifest {file} is invalid: {error}"));
                Entry {
                    agent: Agent::new(manifest.id()),
                    manifest,
                    source: Source::BuiltIn,
                    text: Arc::from(*text),
                }
            })
            .collect();
        Manifests::indexed(entries)
    }

    /// The manifests compiled into this crate as written, by file name: what an app built
    /// against it sends a daemon, which may be older than the app.
    pub fn built_in_sources() -> &'static [(&'static str, &'static str)] {
        BUILT_IN
    }

    /// The built-in manifests, then those the app sent, then the overrides in a directory
    /// (`~/.muster/agent-detection/` in practice). A missing directory is no overrides.
    pub fn load(app: &[(String, String)], overrides: Option<&Path>) -> (Manifests, Vec<Warning>) {
        let mut entries = Manifests::built_in().entries;
        let mut warnings = Vec::new();

        for (name, text) in app {
            let source = Source::App(name.clone());
            match app_manifest(text, &entries) {
                Ok(manifest) => place(&mut entries, manifest, source, text),
                Err(problem) => warnings.push(Warning { source, problem }),
            }
        }

        if let Some(dir) = overrides {
            for (path, read) in override_files(dir, &mut warnings) {
                let source = Source::Override(path.clone());
                let placed = read.and_then(|text| {
                    let manifest = override_manifest(&path, &text)?;
                    Ok((manifest, text))
                });
                match placed {
                    Ok((manifest, text)) => place(&mut entries, manifest, source, &text),
                    Err(problem) => warnings.push(Warning { source, problem }),
                }
            }
        }

        (Manifests::indexed(entries), warnings)
    }

    fn indexed(entries: Vec<Entry>) -> Manifests {
        let mut by_rank: Vec<usize> = (0..entries.len()).collect();
        // A name two manifests both claim goes to the one from the higher layer.
        by_rank.sort_by_key(|&index| std::cmp::Reverse(entries[index].source.rank()));
        let mut names = HashMap::new();
        let mut script_paths = Vec::new();
        for index in by_rank {
            let manifest = &entries[index].manifest;
            for name in
                std::iter::once(manifest.id()).chain(manifest.aliases().iter().map(String::as_str))
            {
                names.entry(lookup_name(name)).or_insert(index);
            }
            for path in manifest.script_paths() {
                script_paths.push((path_components(path), index));
            }
        }
        Manifests { entries, names, script_paths }
    }

    /// Every agent, in no particular order.
    pub fn agents(&self) -> impl Iterator<Item = &Agent> {
        self.entries.iter().map(|entry| &entry.agent)
    }

    /// The agents whose manifest in use differs from the one `before` used: added, removed, or
    /// written differently. A pane running any other agent is detected exactly as it was, so
    /// only these need their detection started over when the manifests are reloaded.
    pub fn changed_since(&self, before: &Manifests) -> Vec<Agent> {
        let mut changed: Vec<Agent> = self
            .entries
            .iter()
            .filter(|entry| before.entry(&entry.agent).is_none_or(|old| old.text != entry.text))
            .map(|entry| entry.agent.clone())
            .collect();
        changed.extend(
            before
                .entries
                .iter()
                .filter(|old| self.entry(&old.agent).is_none())
                .map(|old| old.agent.clone()),
        );
        changed
    }

    /// Where the manifest in use for an agent came from.
    pub fn source(&self, agent: &Agent) -> Option<&Source> {
        self.entry(agent).map(|entry| &entry.source)
    }

    /// The agent a name - an executable, an alias, a `MUSTER_AGENT` value - means, ignoring
    /// case, surrounding space and an executable or script suffix.
    pub fn agent_named(&self, name: &str) -> Option<Agent> {
        self.names.get(&lookup_name(name)).map(|&index| self.entries[index].agent.clone())
    }

    /// The agent whose package a script path lies inside.
    pub fn agent_for_script_path(&self, path: &str) -> Option<Agent> {
        let components = path_components(path);
        self.script_paths
            .iter()
            .find(|(wanted, _)| {
                !wanted.is_empty()
                    && components.windows(wanted.len()).any(|window| window == wanted)
            })
            .map(|&(_, index)| self.entries[index].agent.clone())
    }

    /// An agent's verdict on its pane. No agent is unknown; an agent whose manifest has gone
    /// since it was identified is idle, as a known agent with no matching rule is.
    pub fn detect(&self, agent: Option<&Agent>, input: Input<'_>) -> Detection {
        let Some(agent) = agent else {
            return Detection::unknown();
        };
        match self.entry(agent) {
            Some(entry) => entry.manifest.evaluate(input),
            None => Detection::fallback(State::Idle),
        }
    }

    fn entry(&self, agent: &Agent) -> Option<&Entry> {
        self.entries.iter().find(|entry| &entry.agent == agent)
    }
}

/// Replaces the manifest with the same id, or adds a new agent.
fn place(entries: &mut Vec<Entry>, manifest: Manifest, source: Source, text: &str) {
    let entry = Entry { agent: Agent::new(manifest.id()), manifest, source, text: Arc::from(text) };
    match entries.iter_mut().find(|existing| existing.agent == entry.agent) {
        Some(existing) => *existing = entry,
        None => entries.push(entry),
    }
}

fn gate_engine(text: &str) -> Result<(), String> {
    match Manifest::required_engine(text) {
        Some(needed) if needed > ENGINE_VERSION => Err(format!(
            "it needs detection engine {needed}, and this daemon has engine {ENGINE_VERSION}"
        )),
        _ => Ok(()),
    }
}

/// herdr held a remote manifest to the same terms: versioned, gated, and never older than the
/// manifest it would replace.
fn app_manifest(text: &str, entries: &[Entry]) -> Result<Manifest, String> {
    gate_engine(text)?;
    let manifest = Manifest::parse(text)?;
    let version = manifest.version().ok_or("it has no version")?;
    if Manifest::required_engine(text).is_none() {
        return Err("it has no min_engine_version".to_string());
    }
    if let Some(current) = entries
        .iter()
        .find(|entry| entry.agent.id() == manifest.id())
        .and_then(|entry| entry.manifest.version())
        && version < current
    {
        return Err(format!(
            "its version {version} is older than the {current} the daemon already has"
        ));
    }
    Ok(manifest)
}

fn override_manifest(path: &Path, text: &str) -> Result<Manifest, String> {
    gate_engine(text)?;
    let manifest = Manifest::parse(text)?;
    let stem = path.file_stem().and_then(|stem| stem.to_str()).map(lookup_name);
    let named = std::iter::once(manifest.id())
        .chain(manifest.aliases().iter().map(String::as_str))
        .any(|name| Some(lookup_name(name)) == stem);
    if !named {
        return Err(format!(
            "the file is named for a different agent than its manifest id {}; name it {}.toml",
            manifest.id(),
            manifest.id()
        ));
    }
    Ok(manifest)
}

/// Every `.toml` in the directory, by name so the order is stable, with its contents or why
/// they could not be read.
fn override_files(
    dir: &Path,
    warnings: &mut Vec<Warning>,
) -> Vec<(PathBuf, Result<String, String>)> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(Warning {
                source: Source::Override(dir.to_path_buf()),
                problem: format!("the directory could not be read: {error}"),
            });
            return Vec::new();
        }
    };
    let mut paths: Vec<PathBuf> = listing
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).map_err(|error| error.to_string());
            (path, text)
        })
        .collect()
}

/// herdr's `normalized_agent_lookup_name`: trimmed, lowercased, and without the one suffix an
/// executable or script is most likely to carry.
pub(crate) fn lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

fn path_components(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter(|component| !component.is_empty()).map(lookup_name).collect()
}

#[cfg(test)]
mod tests;
