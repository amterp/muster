use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

/// What installing one harness's adapter takes: the harness's own commands, run in order, and
/// what is left for the person afterwards, a line each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub commands: Vec<Step>,
    pub then: Vec<String>,
}

/// One of a harness's own commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub words: Vec<String>,
    /// A failure that still leaves the command's work done: what the harness says on stderr, and
    /// what that means for the person.
    pub done_if_said: Option<(&'static str, &'static str)>,
}

impl Step {
    pub(crate) fn of(words: &[&str]) -> Step {
        Step { words: words.iter().map(ToString::to_string).collect(), done_if_said: None }
    }

    pub fn line(&self) -> String {
        self.words.iter().map(|word| shell_word(word)).collect::<Vec<_>>().join(" ")
    }
}

impl Plan {
    pub fn printed(&self) -> Vec<String> {
        self.commands.iter().map(Step::line).collect()
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

/// `text` as one word a POSIX shell reads back unchanged, for a command printed to be pasted:
/// as it is when it needs no quoting, which is every path Muster makes, and single-quoted
/// otherwise.
pub fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c));
    if plain { text.to_string() } else { format!("'{}'", text.replace('\'', "'\\''")) }
}

pub const PATIENCE: Duration = Duration::from_mins(2);
pub const REFRESH_PATIENCE: Duration = Duration::from_secs(10);

pub type Run<'a> = dyn FnMut(&[String], Duration) -> io::Result<Option<Output>> + 'a;

pub struct InstallContext<'a> {
    pub extras: &'a Path,
    pub environment: &'a BTreeMap<String, String>,
    pub muster_home: Option<&'a Path>,
}

impl std::fmt::Debug for InstallContext<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstallContext")
            .field("extras", &self.extras)
            .field("muster_home", &self.muster_home)
            .finish_non_exhaustive()
    }
}

pub(crate) fn asset_word(extras: &Path, relative: &str) -> String {
    shell_word(&extras.join(relative).display().to_string())
}
