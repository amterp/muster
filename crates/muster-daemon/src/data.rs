//! The directory the daemon gives its shells from: Ghostty's terminfo entry and its shell
//! integration, which ship beside the daemon rather than inside it
//! (`packaging/muster-daemon-data/README.md`).

use std::path::{Path, PathBuf};

/// The directory's name, and where the daemon looks for it when `--data` does not say: beside
/// its own executable.
pub(crate) const NAME: &str = "muster-daemon-data";

/// What a pane needs from it. Checked once at start, because a pane started without them runs
/// as a terminal no program in it can find.
const REQUIRED: [&str; 7] = [
    "terminfo/x/xterm-ghostty",
    "terminfo/78/xterm-ghostty",
    "shell-integration/bash/ghostty.bash",
    "shell-integration/bash/bash-preexec.sh",
    "shell-integration/zsh/.zshenv",
    "shell-integration/zsh/ghostty-integration",
    "shell-integration/fish/vendor_conf.d/ghostty-shell-integration.fish",
];

#[derive(Debug, Clone)]
pub(crate) struct Data {
    dir: PathBuf,
}

impl Data {
    /// The directory `--data` named, or the one beside the executable. Absolute, since shells
    /// in every other directory are pointed at it. An error is worded to be printed as it
    /// stands.
    pub(crate) fn locate(named: Option<&Path>) -> Result<Data, String> {
        let dir = match named {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_exe()
                .map_err(|error| format!("could not find its own executable: {error}"))?
                .with_file_name(NAME),
        };
        let missing: Vec<&str> =
            REQUIRED.iter().copied().filter(|file| !dir.join(file).is_file()).collect();
        if !missing.is_empty() {
            return Err(format!(
                "{} is not a complete {NAME} directory: it lacks {}. The daemon has not started, \
                 because its panes would run as xterm-ghostty with no terminfo entry for it, which \
                 breaks every full-screen program in them. Install the directory beside the \
                 daemon, or pass --data with its path; a checkout builds it with ./dev -d, at \
                 deps/ghostty/zig-out/{NAME}.",
                dir.display(),
                missing.join(", ")
            ));
        }
        let dir = dir
            .canonicalize()
            .map_err(|error| format!("could not resolve {}: {error}", dir.display()))?;
        Ok(Data { dir })
    }

    pub(crate) fn terminfo(&self) -> PathBuf {
        self.dir.join("terminfo")
    }

    /// Laid out as Ghostty lays out `<resources>/shell-integration`, which its scripts rely on
    /// to find each other.
    pub(crate) fn shell_integration(&self) -> PathBuf {
        self.dir.join("shell-integration")
    }
}
