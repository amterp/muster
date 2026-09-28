//! The directory the daemon gives its shells from: Ghostty's terminfo entry and its shell
//! integration, which ship beside the daemon rather than inside it
//! (`packaging/muster-daemon-data/README.md`).

use std::path::{Path, PathBuf};

/// The directory's name. Where the daemon looks for it when `--data` does not say is
/// [`unnamed`].
pub(crate) const NAME: &str = "muster-daemon-data";

/// What a pane needs from it. Checked once at start, because a pane started without them runs
/// as a terminal no program in it can find.
const REQUIRED: [&str; 9] = [
    "terminfo/x/xterm-ghostty",
    "terminfo/78/xterm-ghostty",
    "shell-integration/bash/ghostty.bash",
    "shell-integration/bash/bash-preexec.sh",
    "shell-integration/zsh/.zshenv",
    "shell-integration/zsh/ghostty-integration",
    "shell-integration/fish/vendor_conf.d/ghostty-shell-integration.fish",
    // What the integration's ssh wrapper runs; without it, `ssh` in a pane fails outright.
    "bin/ghostty",
    // What `muster` is on a PATH a login profile has reset (`spawn::shell_features`).
    "bin/muster",
];

/// How to get a complete directory, for every error that lacks one.
const FIX: &str = "Install the directory beside the daemon (in a bundle, in its \
                   Contents/Resources), or pass --data with its path; a checkout builds it with \
                   ./dev -d, at deps/ghostty/zig-out/muster-daemon-data.";

/// Where the directory is for a daemon at `executable` when `--data` does not say: in the
/// bundle's `Contents/Resources` for one in a bundle's `Contents/MacOS`, where `./dev --bundle`
/// puts it, and beside the executable anywhere else.
fn unnamed(executable: &Path) -> PathBuf {
    let folder = executable.parent();
    let contents = folder.and_then(Path::parent);
    let named = |path: Option<&Path>, name: &str| {
        path.and_then(Path::file_name).is_some_and(|file| file == name)
    };
    match contents {
        Some(contents) if named(folder, "MacOS") && named(Some(contents), "Contents") => {
            contents.join("Resources").join(NAME)
        }
        _ => executable.with_file_name(NAME),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Data {
    dir: PathBuf,
}

impl Data {
    /// The directory `--data` named, or the one [`unnamed`] says. Absolute, since shells
    /// in every other directory are pointed at it. An error is worded to be printed as it
    /// stands.
    pub(crate) fn locate(named: Option<&Path>) -> Result<Data, String> {
        let dir = match named {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_exe()
                .map_err(|error| {
                    format!(
                        "could not find its own executable ({error}), so it cannot look for \
                         {NAME} beside it. The daemon has not started. Pass --data with the \
                         directory's path."
                    )
                })
                .map(|executable| unnamed(&executable))?,
        };
        Data::check(&dir)
    }

    fn check(dir: &Path) -> Result<Data, String> {
        if !dir.is_dir() {
            return Err(format!(
                "{} does not exist, or is not a directory. The daemon has not started, because \
                 its panes would run as xterm-ghostty with no terminfo entry for it. {FIX}",
                dir.display()
            ));
        }
        let missing: Vec<&str> =
            REQUIRED.iter().copied().filter(|file| !dir.join(file).is_file()).collect();
        if !missing.is_empty() {
            return Err(format!(
                "{} is not a complete {NAME} directory: it lacks {}. The daemon has not started, \
                 because its panes would run as xterm-ghostty with no terminfo entry for it, which \
                 breaks every full-screen program in them. {FIX}",
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

    /// Where the integration finds `ghostty`, as `GHOSTTY_BIN_DIR` tells it, and what its `path`
    /// feature appends to a pane's PATH: Muster's stand-in for its ssh features, and `muster`.
    pub(crate) fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// Laid out as Ghostty lays out `<resources>/shell-integration`, which its scripts rely on
    /// to find each other.
    pub(crate) fn shell_integration(&self) -> PathBuf {
        self.dir.join("shell-integration")
    }
}

/// A data directory taken as it is, for a unit test whose panes never start.
#[cfg(test)]
impl Data {
    pub(crate) fn unchecked(dir: PathBuf) -> Data {
        Data { dir }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_built_here_finds_it_beside_itself() {
        assert_eq!(
            unnamed(Path::new("/src/muster/target/debug/muster-daemon")),
            Path::new("/src/muster/target/debug/muster-daemon-data")
        );
    }

    #[test]
    fn a_daemon_in_a_bundle_finds_it_in_the_bundles_resources() {
        // Where `./dev --bundle` puts it, since a bundle keeps its executables apart from
        // everything else. `muster-daemon replace` named no directory, and a handoff by hand in
        // an installed Muster failed for want of one.
        assert_eq!(
            unnamed(Path::new(
                "/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/MacOS/\
                 muster-daemon"
            )),
            Path::new(
                "/Applications/Muster.app/Contents/Library/MusterSessions.app/Contents/Resources/\
                 muster-daemon-data"
            )
        );
    }

    #[test]
    fn a_missing_directory_is_called_missing() {
        let dir = std::env::temp_dir().join("muster-data-that-does-not-exist");
        let error = Data::locate(Some(&dir)).unwrap_err();
        assert!(error.starts_with(&format!("{} does not exist", dir.display())), "{error}");
        assert!(!error.contains("lacks"), "{error}");
    }

    #[test]
    fn an_incomplete_directory_names_what_it_lacks() {
        let error = Data::locate(Some(&std::env::temp_dir())).unwrap_err();
        assert!(error.contains("is not a complete muster-daemon-data directory"), "{error}");
        assert!(error.contains("terminfo/x/xterm-ghostty"), "{error}");
    }
}
